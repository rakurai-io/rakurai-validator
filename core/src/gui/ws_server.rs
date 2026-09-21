use {
    crate::gui::{
        gui_snapshot::{
            format_startup_progress_running_message, format_stub_self_peer_message,
            GuiContext, GuiWsSnapshotRequest,
        },
        metrics::format_boot_progress_running_message,
        slot_query::{GuiWsQuery, GuiWsRankingsQuery},
    },
    axum::{
        extract::{
            connect_info::ConnectInfo,
            ws::{Message, WebSocket, WebSocketUpgrade},
            State,
        },
        http::StatusCode,
        response::IntoResponse,
        routing::get,
        Router,
    },
    futures::{SinkExt, StreamExt},
    std::{
        collections::HashSet,
        net::{IpAddr, SocketAddr},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, RwLock,
        },
        time::Duration,
    },
    tokio::sync::{broadcast, mpsc, oneshot},
};

pub type GuiIpWhitelist = Arc<RwLock<HashSet<IpAddr>>>;

const WS_SERVER_LOOP_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone)]
struct WsServerState {
    sender: broadcast::Sender<String>,
    query_sender: mpsc::Sender<GuiWsQuery>,
    rankings_sender: mpsc::Sender<GuiWsRankingsQuery>,
    snapshot_sender: mpsc::Sender<GuiWsSnapshotRequest>,
    gui_context: GuiContext,
    connections: Arc<AtomicUsize>,
    max_connections: usize,
    ip_whitelist: GuiIpWhitelist,
}

fn is_ip_whitelisted(ip_whitelist: &HashSet<IpAddr>, ip: IpAddr) -> bool {
    !ip_whitelist.is_empty() && ip_whitelist.contains(&ip)
}

pub fn new_broadcast_sender() -> broadcast::Sender<String> {
    let (sender, _) = broadcast::channel(256);
    sender
}

pub async fn serve(
    listen_addr: &str,
    exit: Arc<AtomicBool>,
    sender: broadcast::Sender<String>,
    query_sender: mpsc::Sender<GuiWsQuery>,
    rankings_sender: mpsc::Sender<GuiWsRankingsQuery>,
    snapshot_sender: mpsc::Sender<GuiWsSnapshotRequest>,
    gui_context: GuiContext,
    max_connections: usize,
    ip_whitelist: GuiIpWhitelist,
) {
    let state = WsServerState {
        sender,
        query_sender,
        rankings_sender,
        snapshot_sender,
        gui_context,
        connections: Arc::new(AtomicUsize::new(0)),
        max_connections,
        ip_whitelist,
    };
    let app = Router::new()
        .route("/websocket", get(ws_handler))
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(listen_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            log::error!("failed to bind gui websocket server on {listen_addr}: {err}");
            return;
        }
    };
    log::info!("gui websocket server listening on {listen_addr}");

    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    );
    tokio::select! {
        result = server => {
            if let Err(err) = result {
                log::error!("gui websocket server exited with error: {err}");
            }
        }
        () = async {
            log::info!("gui websocket server loop started");
            while !exit.load(Ordering::Relaxed) {
                tokio::time::sleep(WS_SERVER_LOOP_INTERVAL).await;
            }
            log::info!("gui websocket server loop exited");
        } => {}
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<WsServerState>,
) -> impl IntoResponse {
    let peer_ip = addr.ip();
    if !is_ip_whitelisted(&state.ip_whitelist.read().unwrap(), peer_ip) {
        log::warn!("rejecting gui websocket connection from {peer_ip}: not whitelisted");
        return StatusCode::FORBIDDEN.into_response();
    }

    let active = state.connections.fetch_add(1, Ordering::AcqRel);
    if active >= state.max_connections {
        state.connections.fetch_sub(1, Ordering::AcqRel);
        log::warn!(
            "rejecting gui websocket connection from {addr}: max connections ({max}) reached",
            max = state.max_connections
        );
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    log::info!(
        "gui websocket client connected from {addr} ({active}/{max} active)",
        active = active + 1,
        max = state.max_connections
    );

    ws.protocols(["compress-zstd"]).on_upgrade(move |socket| async move {
        handle_socket(
            socket,
            state.sender.clone(),
            state.query_sender.clone(),
            state.rankings_sender.clone(),
            state.snapshot_sender.clone(),
            state.gui_context.clone(),
        )
        .await;
        state.connections.fetch_sub(1, Ordering::AcqRel);
    })
}

async fn handle_socket(
    socket: WebSocket,
    sender: broadcast::Sender<String>,
    query_sender: mpsc::Sender<GuiWsQuery>,
    rankings_sender: mpsc::Sender<GuiWsRankingsQuery>,
    snapshot_sender: mpsc::Sender<GuiWsSnapshotRequest>,
    gui_context: GuiContext,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    for message in [
        format_boot_progress_running_message(),
        format_startup_progress_running_message(&gui_context),
        format_stub_self_peer_message(&gui_context.identity_pubkey()),
    ]
    .into_iter()
    .flatten()
    {
        if ws_sender
            .send(Message::Text(message.into()))
            .await
            .is_err()
        {
            return;
        }
    }

    if let Some(messages) = request_connect_snapshot(&snapshot_sender).await {
        for message in messages {
            if ws_sender.send(Message::Text(message.into())).await.is_err() {
                return;
            }
        }
    }

    let mut receiver = sender.subscribe();

    loop {
        tokio::select! {
            message = receiver.recv() => {
                match message {
                    Ok(payload) => {
                        if ws_sender.send(Message::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = ws_receiver.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Text(text))) => {
                        if let Some(response) =
                            handle_inbound_message(text.as_ref(), &query_sender, &rankings_sender)
                                .await
                        {
                            if ws_sender.send(Message::Text(response.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct InboundWsMessage {
    topic: String,
    key: String,
    id: Option<u64>,
    params: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct SlotQueryParams {
    slot: u64,
}

async fn request_connect_snapshot(
    snapshot_sender: &mpsc::Sender<GuiWsSnapshotRequest>,
) -> Option<Vec<String>> {
    let (reply_tx, reply_rx) = oneshot::channel();
    snapshot_sender
        .send(GuiWsSnapshotRequest { reply: reply_tx })
        .await
        .ok()?;
    reply_rx.await.ok()
}

async fn handle_inbound_message(
    text: &str,
    query_sender: &mpsc::Sender<GuiWsQuery>,
    rankings_sender: &mpsc::Sender<GuiWsRankingsQuery>,
) -> Option<String> {
    let message: InboundWsMessage = serde_json::from_str(text).ok()?;
    if message.topic != "slot" {
        return None;
    }

    if message.key == "query_rankings" {
        let id = message.id.unwrap_or(0);
        let (reply_tx, reply_rx) = oneshot::channel();
        rankings_sender
            .send(GuiWsRankingsQuery {
                id,
                reply: reply_tx,
            })
            .await
            .ok()?;
        return reply_rx.await.ok();
    }

    if message.key != "query_transactions" {
        return None;
    }

    let params: SlotQueryParams = message
        .params
        .and_then(|params| serde_json::from_value(params).ok())?;
    let id = message.id.unwrap_or(0);
    let (reply_tx, reply_rx) = oneshot::channel();
    query_sender
        .send(GuiWsQuery {
            slot: params.slot,
            id,
            reply: reply_tx,
        })
        .await
        .ok()?;
    reply_rx.await.ok()
}
