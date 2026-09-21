use {
    super::{
        PostPackConfirmation, PostPackConfirmationConfig, PostPackConfirmationConfigStatus,
        decision_maker::{BufferedPacketsDecision, DecisionMaker},
        postpack_confirmation_config::load_postpack_confirmation_config,
        reward_distributor::LatestBankPair,
    },
    crate::proxy::p2c_auto_config::{
        P2C_AUTOCONFIG_METRICS, autoconfig_ranked_candidates, get_endpoint,
    },
    arc_swap::ArcSwap,
    crossbeam_channel::{Receiver, Sender, unbounded},
    log::{error, info, warn},
    p2c_protos::pre_conf::{
        auth::{Token, auth_service_client::AuthServiceClient},
        block_engine::{
            ExpiringPacketBatch, P2cUpdateCount, PacketBatchUpdate,
            block_engine_relayer_client::BlockEngineRelayerClient, packet_batch_update::Msg,
        },
        packet::{Meta as ProtoMeta, Packet as ProtoPacket, PacketBatch as ProtoPacketBatch},
        shared::{Header, Heartbeat},
    },
    prost_types::Timestamp,
    serde::{Deserialize, Serialize},
    solana_gossip::cluster_info::ClusterInfo,
    solana_keypair::Keypair,
    solana_message::v0::LoadedAddresses,
    solana_perf::packet::BytesPacket,
    solana_pubkey::Pubkey,
    solana_runtime_transaction::transaction_with_meta::TransactionWithMeta,
    solana_signature::Signature,
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
    std::{
        collections::{HashMap, HashSet},
        sync::{
            Arc, Mutex, RwLock,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::{Duration, Instant, SystemTime},
    },
    tokio::{
        runtime::Handle,
        sync::mpsc,
        time::{self, interval},
    },
    tokio_stream::wrappers::ReceiverStream,
    tonic::{
        Request, Status,
        codegen::InterceptedService,
        service::Interceptor,
        transport::{Channel, Endpoint},
    },
};

#[cfg(feature = "build_validator")]
unsafe extern "C" {
    #[allow(improper_ctypes)]
    #[allow(improper_ctypes_definitions)]
    pub fn clear_postpack_conf_signatures();
}

struct P2cAuthInterceptor {
    access_token: Arc<ArcSwap<Token>>,
}

impl P2cAuthInterceptor {
    fn new(access_token: Arc<ArcSwap<Token>>) -> Self {
        Self { access_token }
    }
}

impl Interceptor for P2cAuthInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request.metadata_mut().insert(
            "authorization",
            format!("Bearer {}", self.access_token.load().value)
                .parse()
                .map_err(|_| Status::invalid_argument("Failed to parse authorization token"))?,
        );
        Ok(request)
    }
}

fn tin_connection_state_log(
    url: String,
    actual_url: String,
    uuid: String,
    primary: bool,
    state: String,
) {
    #[cfg(feature = "build_validator")]
    {
        if unsafe { super::rakurai_enabled() } {
            let name: &'static str = "rakurai_tin_connection_state";
            let datapoint = solana_metrics::create_datapoint!(
                @point name,
                ("url", url, String),
                ("actual_url", actual_url, String),
                ("uuid", uuid, String),
                ("source", "p2c", String),
                ("primary", primary, bool),
                ("state", state, String),
            );
            solana_metrics::submit(datapoint, log::Level::Info);
            return;
        }
    }
    let _ = (url, actual_url, uuid, primary, state);
}

fn publish_tin_log_epoch(tin_log_epochs: &Mutex<HashMap<String, Arc<AtomicU64>>>, epoch: u64) {
    for shared_epoch in tin_log_epochs.lock().unwrap().values() {
        shared_epoch.store(epoch, Ordering::Relaxed);
    }
}

fn maybe_log_tin_connection_for_epoch(
    shared_epoch: &AtomicU64,
    local_epoch: &mut u64,
    url: &str,
    actual_url: &str,
    uuid: &str,
) {
    let epoch = shared_epoch.load(Ordering::Relaxed);
    if epoch != 0 && epoch != *local_epoch {
        *local_epoch = epoch;
        tin_connection_state_log(
            url.to_string(),
            actual_url.to_string(),
            uuid.to_string(),
            false,
            "connected".to_string(),
        );
    }
}

fn p2c_update_count_log(msg: &P2cUpdateCount) {
    if msg.total_count == 0 {
        return;
    }
    #[cfg(feature = "build_validator")]
    {
        if unsafe { super::rakurai_enabled() } {
            let name: &'static str = "rakurai_p2c_update_count";
            let datapoint = solana_metrics::create_datapoint!(
                @point name,
                ("uuid", msg.uuid.clone(), String),
                ("slot", msg.slot, i64),
                ("scheduler_count", msg.scheduler_count, i64),
                ("tpu_count", msg.tpu_count, i64),
                ("total_count", msg.total_count, i64),
                ("p2c_tpu_enabled", msg.p2c_tpu_enabled, bool),
            );
            solana_metrics::submit(datapoint, log::Level::Info);
            return;
        }
    }
    let _ = msg;
}

fn p2c_total_update_count_log(slot: u64, scheduler_count: u64, tpu_count: u64) {
    let total_count = scheduler_count.saturating_add(tpu_count);
    if total_count == 0 {
        return;
    }
    #[cfg(feature = "build_validator")]
    {
        if unsafe { super::rakurai_enabled() } {
            let name: &'static str = "rakurai_p2c_total_update_count";
            let datapoint = solana_metrics::create_datapoint!(
                @point name,
                ("slot", slot, i64),
                ("scheduler_count", scheduler_count, i64),
                ("tpu_count", tpu_count, i64),
                ("total_count", total_count, i64),
            );
            solana_metrics::submit(datapoint, log::Level::Info);
            return;
        }
    }
    let _ = (slot, scheduler_count, tpu_count);
}

/// Validator-wide per-slot tin update counts (pre fan-out). Metrics only — no gRPC.
struct P2cTotalUpdateCountTracker {
    slot: Option<u64>,
    scheduler_count: u64,
    tpu_count: u64,
}

impl P2cTotalUpdateCountTracker {
    fn new() -> Self {
        Self {
            slot: None,
            scheduler_count: 0,
            tpu_count: 0,
        }
    }

    fn working_slot(shared_bank_update: &Arc<RwLock<LatestBankPair>>) -> Option<u64> {
        shared_bank_update
            .read()
            .ok()
            .map(|bank_pair| bank_pair.working_bank.slot())
    }

    fn ensure_slot(&mut self, slot: u64) {
        match self.slot {
            Some(current) if current != slot => {
                self.flush();
                self.slot = Some(slot);
                self.scheduler_count = 0;
                self.tpu_count = 0;
            }
            None => {
                self.slot = Some(slot);
            }
            _ => {}
        }
    }

    fn on_scheduler_processed(
        &mut self,
        shared_bank_update: &Arc<RwLock<LatestBankPair>>,
        count: u64,
    ) {
        if let Some(slot) = Self::working_slot(shared_bank_update) {
            self.ensure_slot(slot);
        }
        self.scheduler_count = self.scheduler_count.saturating_add(count);
    }

    fn on_tpu_processed(&mut self, shared_bank_update: &Arc<RwLock<LatestBankPair>>, count: u64) {
        if let Some(slot) = Self::working_slot(shared_bank_update) {
            self.ensure_slot(slot);
        }
        self.tpu_count = self.tpu_count.saturating_add(count);
    }

    fn poll_slot(&mut self, shared_bank_update: &Arc<RwLock<LatestBankPair>>) {
        if let Some(slot) = Self::working_slot(shared_bank_update) {
            self.ensure_slot(slot);
        }
    }

    fn flush(&mut self) {
        let Some(slot) = self.slot else {
            return;
        };
        p2c_total_update_count_log(slot, self.scheduler_count, self.tpu_count);
    }
}

/// Per-connection, per-slot send counters flushed to metrics and optionally gRPC.
struct P2cUpdateCountTracker {
    uuid: String,
    p2c_tpu_enabled: bool,
    slot: Option<u64>,
    scheduler_count: u64,
    tpu_count: u64,
    /// None when the consumer does not implement StartP2cUpdateCountStream.
    count_grpc_sender: Option<mpsc::Sender<P2cUpdateCount>>,
}

impl P2cUpdateCountTracker {
    fn new(
        uuid: String,
        p2c_tpu_enabled: bool,
        count_grpc_sender: Option<mpsc::Sender<P2cUpdateCount>>,
    ) -> Self {
        Self {
            uuid,
            p2c_tpu_enabled,
            slot: None,
            scheduler_count: 0,
            tpu_count: 0,
            count_grpc_sender,
        }
    }

    fn working_slot(shared_bank_update: &Arc<RwLock<LatestBankPair>>) -> Option<u64> {
        shared_bank_update
            .read()
            .ok()
            .map(|bank_pair| bank_pair.working_bank.slot())
    }

    async fn ensure_slot(&mut self, slot: u64) {
        match self.slot {
            Some(current) if current != slot => {
                self.flush().await;
                self.slot = Some(slot);
                self.scheduler_count = 0;
                self.tpu_count = 0;
            }
            None => {
                self.slot = Some(slot);
            }
            _ => {}
        }
    }

    async fn on_scheduler_sent(&mut self, shared_bank_update: &Arc<RwLock<LatestBankPair>>) {
        if let Some(slot) = Self::working_slot(shared_bank_update) {
            self.ensure_slot(slot).await;
        }
        self.scheduler_count = self.scheduler_count.saturating_add(1);
    }

    async fn on_tpu_sent(&mut self, shared_bank_update: &Arc<RwLock<LatestBankPair>>) {
        if let Some(slot) = Self::working_slot(shared_bank_update) {
            self.ensure_slot(slot).await;
        }
        self.tpu_count = self.tpu_count.saturating_add(1);
    }

    async fn poll_slot(&mut self, shared_bank_update: &Arc<RwLock<LatestBankPair>>) {
        if let Some(slot) = Self::working_slot(shared_bank_update) {
            self.ensure_slot(slot).await;
        }
    }

    async fn flush(&mut self) {
        let Some(slot) = self.slot else {
            return;
        };
        let total_count = self.scheduler_count.saturating_add(self.tpu_count);
        let msg = P2cUpdateCount {
            uuid: self.uuid.clone(),
            slot,
            scheduler_count: self.scheduler_count,
            tpu_count: self.tpu_count,
            total_count,
            p2c_tpu_enabled: self.p2c_tpu_enabled,
        };
        p2c_update_count_log(&msg);
        if let Some(sender) = &self.count_grpc_sender {
            // Best-effort: never fail the packet stream if the count consumer is gone.
            let _ = sender.send(msg).await;
        }
    }
}

const GRPC_QUEUE_CAPACITY: usize = 1_000;
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(5_000);
const SLOT_POLL_INTERVAL: Duration = Duration::from_millis(5);
const AUTH_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
const AUTH_CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const AUTH_REFRESH_WITHIN_S: u64 = 5 * 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerUpdate {
    pub txn: VersionedTransaction,
    pub loaded_addresses: LoadedAddresses,
    pub packet: BytesPacket,
}
/// Transactions scheduled for consume; converted to gRPC updates in the notifier thread.
pub struct SchedulerUpdateWork<Tx> {
    pub transactions: Vec<Tx>,
    pub packet_batches: Vec<BytesPacket>,
    /// Mev + ReSell hashes, parallel to `packet_batches` (stringified into `Meta.addr` on send).
    pub hash_addrs: Vec<crate::banking_stage::PostPackConfHashes>,
}

pub type SerializedSchedulerUpdate = (BytesPacket, solana_blake3_hasher::Hash);
pub type SerializedTpuUpdate = (BytesPacket, Signature);

/// P2C update channel sender paired with the forward gate set by reward distributor.
#[derive(Clone)]
pub struct P2cUpdateSender {
    pub sender: Sender<SerializedTpuUpdate>,
    pub forward_to_p2c: Arc<AtomicBool>,
}

impl P2cUpdateSender {
    pub fn unbounded() -> (Self, Receiver<SerializedTpuUpdate>) {
        let forward_to_p2c = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = unbounded();
        (
            Self {
                sender,
                forward_to_p2c,
            },
            receiver,
        )
    }
}

pub struct SchedulerUpdateNotifier<Tx> {
    decision_maker: DecisionMaker,
    shared_bank_update: Arc<RwLock<LatestBankPair>>,
    cluster_info: Option<Arc<ClusterInfo>>,
    vote_account: Pubkey,
    /// Admin-settable entries merged with on-chain ones in `sync_postpack_confirmation_config`.
    /// Owned locally: this notifier is the only consumer, so it doesn't need to be shared.
    postpack_confirmation_config: PostPackConfirmationConfig,
    postpack_confirmation_active_entries: Arc<ArcSwap<PostPackConfirmationConfigStatus>>,
    post_pack_confirmation_uuid_blocklist: Arc<ArcSwap<Vec<String>>>,
    receiver: Receiver<SchedulerUpdateWork<Tx>>,
    p2c_update_receiver: Option<Receiver<SerializedTpuUpdate>>,
    exit: Arc<AtomicBool>,
}

impl<Tx> SchedulerUpdateNotifier<Tx>
where
    Tx: TransactionWithMeta + Send + 'static,
{
    pub fn new(
        decision_maker: DecisionMaker,
        shared_bank_update: Arc<RwLock<LatestBankPair>>,
        cluster_info: Option<Arc<ClusterInfo>>,
        vote_account: Pubkey,
        postpack_confirmation_active_entries: Arc<ArcSwap<PostPackConfirmationConfigStatus>>,
        post_pack_confirmation_uuid_blocklist: Arc<ArcSwap<Vec<String>>>,
        receiver: Receiver<SchedulerUpdateWork<Tx>>,
        p2c_update_receiver: Option<Receiver<SerializedTpuUpdate>>,
        exit: Arc<AtomicBool>,
    ) -> Self {
        Self {
            decision_maker,
            shared_bank_update,
            cluster_info,
            vote_account,
            postpack_confirmation_config: PostPackConfirmationConfig::default(),
            postpack_confirmation_active_entries,
            post_pack_confirmation_uuid_blocklist,
            receiver,
            p2c_update_receiver,
            exit,
        }
    }

    pub fn run(&mut self) {
        // Background tokio workers run gRPC connections while this thread blocks on recv.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .thread_name("solSchedTokio")
            .enable_all()
            .build()
            .expect("failed to create tokio runtime for SchedulerUpdateNotifier");
        let handle = rt.handle().clone();

        let Some(cluster_info) = self.cluster_info.clone() else {
            warn!(
                "SchedulerUpdateNotifier: cluster_info unavailable; postpack-confirmation gRPC disabled"
            );
            return;
        };

        let mut current_entries: Vec<PostPackConfirmation> = Vec::new();
        let mut connection_tasks = Vec::new();
        let task_exits = Arc::new(Mutex::new(HashMap::<String, Arc<AtomicBool>>::new()));
        let tin_log_epochs = Arc::new(Mutex::new(HashMap::<String, Arc<AtomicU64>>::new()));
        let update_senders = Arc::new(Mutex::new(HashMap::<String, EndpointSender>::new()));
        let mut last_published_tin_log_epoch = super::current_tin_connection_log_epoch();

        sync_postpack_confirmation_config(
            &handle,
            &self.shared_bank_update,
            &self.vote_account,
            &self.postpack_confirmation_config,
            &self.postpack_confirmation_active_entries,
            &self.post_pack_confirmation_uuid_blocklist,
            &cluster_info,
            &self.exit,
            &task_exits,
            &tin_log_epochs,
            &update_senders,
            &mut connection_tasks,
            &mut current_entries,
        );

        let mut last_postpack_sync = Instant::now();
        const POSTPACK_SYNC_INTERVAL: Duration = Duration::from_secs(30);
        let mut total_update_tracker = P2cTotalUpdateCountTracker::new();
        // Seed current slot so idle slots still get dumped on roll.
        total_update_tracker.poll_slot(&self.shared_bank_update);
        let mut last_total_slot_poll = Instant::now();

        while !self.exit.load(Ordering::Relaxed) {
            if let Some(p2c_update_receiver) = &self.p2c_update_receiver {
                while let Ok(update) = p2c_update_receiver.try_recv() {
                    let senders = update_senders.lock().unwrap().clone();
                    let had_tpu_endpoint = senders.values().any(|e| e.tpu_sender.is_some());
                    dispatch_serialized_scheduler_update(
                        cluster_info.keypair().clone(),
                        &senders,
                        update,
                    );
                    if had_tpu_endpoint {
                        total_update_tracker.on_tpu_processed(&self.shared_bank_update, 1);
                    }
                }
            }

            while let Ok(work) = self.receiver.try_recv() {
                let packet_count = work.packet_batches.len() as u64;
                let senders = update_senders.lock().unwrap().clone();
                let had_endpoints = !senders.is_empty();
                dispatch_scheduler_update_work(&senders, work);
                if had_endpoints {
                    total_update_tracker
                        .on_scheduler_processed(&self.shared_bank_update, packet_count);
                }
            }

            if last_total_slot_poll.elapsed() >= SLOT_POLL_INTERVAL {
                total_update_tracker.poll_slot(&self.shared_bank_update);
                last_total_slot_poll = Instant::now();
            }

            if last_postpack_sync.elapsed() >= POSTPACK_SYNC_INTERVAL {
                let tin_log_epoch = crate::banking_stage::current_tin_connection_log_epoch();
                if tin_log_epoch != 0 && tin_log_epoch != last_published_tin_log_epoch {
                    last_published_tin_log_epoch = tin_log_epoch;
                    publish_tin_log_epoch(&tin_log_epochs, tin_log_epoch);
                }
                last_postpack_sync = Instant::now();
                let (decision, _, _) = self.decision_maker.make_consume_or_forward_decision();
                match decision {
                    BufferedPacketsDecision::Consume(_) => {}
                    _ => {
                        sync_postpack_confirmation_config(
                            &handle,
                            &self.shared_bank_update,
                            &self.vote_account,
                            &self.postpack_confirmation_config,
                            &self.postpack_confirmation_active_entries,
                            &self.post_pack_confirmation_uuid_blocklist,
                            &cluster_info,
                            &self.exit,
                            &task_exits,
                            &tin_log_epochs,
                            &update_senders,
                            &mut connection_tasks,
                            &mut current_entries,
                        );
                        #[cfg(feature = "build_validator")]
                        unsafe {
                            clear_postpack_conf_signatures();
                        }
                        connection_tasks.retain(|task| !task.is_finished());
                    }
                }
            }
        }

        for task_exit in task_exits.lock().unwrap().drain().map(|(_, v)| v) {
            task_exit.store(true, Ordering::Relaxed);
        }
        rt.block_on(async {
            for task in connection_tasks {
                let _ = task.await;
            }
        });
        // Flush in-progress slot totals on exit so the last slot isn't dropped.
        total_update_tracker.flush();
        log::info!("SchedulerUpdateNotifier: exiting");
    }
}

#[derive(Clone)]
struct EndpointSender {
    /// `StartExpiringPacketStream` — ReSell hash, no TPU (None when `resell` is false).
    resell_sender: Option<mpsc::Sender<SerializedSchedulerUpdate>>,
    /// `StartExpiringMevPacketStream` — Mev hash / boost (None when `mev` is false).
    mev_sender: Option<mpsc::Sender<SerializedSchedulerUpdate>>,
    /// `StartExpiringTpuPacketStream` — Mev only when flag is set (None when disabled).
    tpu_sender: Option<mpsc::Sender<SerializedTpuUpdate>>,
    url: String,
    mev: bool,
    resell: bool,
    enable_tpu_p2c_update: bool,
}

fn sync_postpack_confirmation_config(
    handle: &Handle,
    shared_bank_update: &Arc<RwLock<LatestBankPair>>,
    vote_account: &Pubkey,
    postpack_confirmation_config: &PostPackConfirmationConfig,
    postpack_confirmation_active_entries: &Arc<ArcSwap<PostPackConfirmationConfigStatus>>,
    post_pack_confirmation_uuid_blocklist: &Arc<ArcSwap<Vec<String>>>,
    cluster_info: &Arc<ClusterInfo>,
    exit: &Arc<AtomicBool>,
    task_exits: &Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    tin_log_epochs: &Arc<Mutex<HashMap<String, Arc<AtomicU64>>>>,
    update_senders: &Arc<Mutex<HashMap<String, EndpointSender>>>,
    connection_tasks: &mut Vec<tokio::task::JoinHandle<()>>,
    current_entries: &mut Vec<PostPackConfirmation>,
) {
    let pda_entries = shared_bank_update
        .read()
        .ok()
        .and_then(|bank_pair| {
            load_postpack_confirmation_config(bank_pair.working_bank.as_ref(), vote_account)
        })
        .map(|config| config.entries)
        .unwrap_or_default();

    let admin_entries = postpack_confirmation_config.entries.clone();
    let blocklisted_uuids = post_pack_confirmation_uuid_blocklist
        .load()
        .as_ref()
        .clone();
    let merged_entries = union_postpack_confirmation_entries(&pda_entries, &admin_entries);
    let blocklisted_entries = blocklisted_entries_from_merged(&merged_entries, &blocklisted_uuids);
    let active_entries = filter_blocklisted_uuids(&merged_entries, &blocklisted_uuids);
    let blocklist: HashSet<String> = blocklisted_uuids.iter().cloned().collect();

    postpack_confirmation_active_entries.store(Arc::new(PostPackConfirmationConfigStatus {
        admin_entries: admin_entries.clone(),
        onchain_entries: pda_entries.clone(),
        blocklisted_uuids,
        blocklisted_entries,
        active_entries: active_entries.clone(),
    }));

    // One connection per UUID (streams multiplexed on that channel).
    let mut keys_to_remove: HashSet<String> = current_entries
        .iter()
        .filter(|entry| !active_entries.iter().any(|a| a.uuid == entry.uuid))
        .map(|entry| entry.uuid.clone())
        .collect();

    {
        let senders = update_senders.lock().unwrap();
        for (uuid, endpoint) in senders.iter() {
            if blocklist.contains(uuid) {
                keys_to_remove.insert(uuid.clone());
                continue;
            }
            let Some(active) = active_entries.iter().find(|entry| entry.uuid == *uuid) else {
                keys_to_remove.insert(uuid.clone());
                continue;
            };
            let active_tpu = active.enable_tpu_p2c_update && active.mev;
            if active.url != endpoint.url
                || active.mev != endpoint.mev
                || active.resell != endpoint.resell
                || active_tpu != endpoint.enable_tpu_p2c_update
            {
                keys_to_remove.insert(uuid.clone());
                continue;
            }
        }
    }

    let entries_to_add: Vec<PostPackConfirmation> = {
        let senders = update_senders.lock().unwrap();
        active_entries
            .iter()
            .filter(|entry| !senders.contains_key(&entry.uuid))
            .cloned()
            .collect()
    };

    if keys_to_remove.is_empty() && entries_to_add.is_empty() {
        *current_entries = active_entries;
        return;
    }

    for key in keys_to_remove {
        if let Some(task_exit) = task_exits.lock().unwrap().remove(&key) {
            task_exit.store(true, Ordering::Relaxed);
        }
        tin_log_epochs.lock().unwrap().remove(&key);
        update_senders.lock().unwrap().remove(&key);
    }

    for entry in entries_to_add {
        let uuid = entry.uuid.clone();
        let url = entry.url.clone();
        let mev = entry.mev;
        let resell = entry.resell;
        let enable_tpu_p2c_update = entry.enable_tpu_p2c_update && mev;
        if !mev && !resell {
            warn!(
                "SchedulerUpdateNotifier: skipping endpoint uuid={uuid} url={url}: \
                 neither mev nor resell enabled"
            );
            continue;
        }

        let (resell_sender, resell_receiver) = if resell {
            let (tx, rx) = mpsc::channel(GRPC_QUEUE_CAPACITY);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let (mev_sender, mev_receiver) = if mev {
            let (tx, rx) = mpsc::channel(GRPC_QUEUE_CAPACITY);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let (tpu_sender, tpu_receiver) = if enable_tpu_p2c_update {
            let (tx, rx) = mpsc::channel(GRPC_QUEUE_CAPACITY);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let task_exit = Arc::new(AtomicBool::new(false));
        let shared_tin_log_epoch =
            Arc::new(AtomicU64::new(super::current_tin_connection_log_epoch()));

        task_exits
            .lock()
            .unwrap()
            .insert(uuid.clone(), task_exit.clone());
        tin_log_epochs
            .lock()
            .unwrap()
            .insert(uuid.clone(), shared_tin_log_epoch.clone());
        update_senders.lock().unwrap().insert(
            uuid.clone(),
            EndpointSender {
                resell_sender,
                mev_sender,
                tpu_sender,
                url: url.clone(),
                mev,
                resell,
                enable_tpu_p2c_update,
            },
        );

        let cluster_info = cluster_info.clone();
        let exit = exit.clone();
        let shared_bank_update = shared_bank_update.clone();
        connection_tasks.push(handle.spawn(async move {
            run_postpack_confirmation_connection(
                url,
                uuid,
                resell,
                mev,
                enable_tpu_p2c_update,
                cluster_info,
                shared_bank_update,
                resell_receiver,
                mev_receiver,
                tpu_receiver,
                task_exit,
                shared_tin_log_epoch,
                exit,
            )
            .await;
        }));
    }

    *current_entries = active_entries;
}

fn dispatch_scheduler_update_work<Tx>(
    senders: &HashMap<String, EndpointSender>,
    work: SchedulerUpdateWork<Tx>,
) where
    Tx: TransactionWithMeta,
{
    if senders.is_empty() {
        return;
    }

    for (packet, hash_addrs) in work.packet_batches.into_iter().zip(work.hash_addrs) {
        for endpoint in senders.values() {
            if let Some(resell_sender) = &endpoint.resell_sender {
                if let Err(error) = resell_sender.try_send((packet.clone(), hash_addrs.resell)) {
                    warn!(
                        "SchedulerUpdateNotifier: failed to enqueue resell update for {}: {error}",
                        endpoint.url
                    );
                }
            }
            if let Some(mev_sender) = &endpoint.mev_sender {
                if let Err(error) = mev_sender.try_send((packet.clone(), hash_addrs.mev.clone())) {
                    warn!(
                        "SchedulerUpdateNotifier: failed to enqueue mev update for {}: {error}",
                        endpoint.url
                    );
                }
            }
        }
    }
}

fn dispatch_serialized_scheduler_update(
    keypair: Arc<Keypair>,
    senders: &HashMap<String, EndpointSender>,
    update: SerializedTpuUpdate,
) {
    if senders.is_empty() {
        return;
    }

    // Sign the txn-sig string; keep the proof Signature until wire (Meta.addr = proof.to_string()).
    let txn_sig = update.1.to_string();
    let proof = keypair.sign_message(txn_sig.as_bytes());
    let update = (update.0, proof);
    for endpoint in senders.values() {
        let Some(tpu_sender) = &endpoint.tpu_sender else {
            continue;
        };
        if let Err(error) = tpu_sender.try_send(update.clone()) {
            warn!(
                "SchedulerUpdateNotifier: failed to enqueue TPU update for {}: {error}",
                endpoint.url
            );
        }
    }
}

#[allow(dead_code)]
fn scheduler_updates_from_work<Tx>(work: SchedulerUpdateWork<Tx>) -> Vec<SchedulerUpdate>
where
    Tx: TransactionWithMeta,
{
    work.transactions
        .into_iter()
        .zip(work.packet_batches.into_iter())
        .map(|(tx, packet)| {
            let sanitized = tx.as_sanitized_transaction();
            SchedulerUpdate {
                txn: sanitized.to_versioned_transaction(),
                loaded_addresses: sanitized.get_loaded_addresses(),
                packet: packet,
            }
        })
        .collect()
}

async fn run_postpack_confirmation_connection(
    url: String,
    uuid: String,
    resell: bool,
    mev: bool,
    enable_tpu_p2c_update: bool,
    cluster_info: Arc<ClusterInfo>,
    shared_bank_update: Arc<RwLock<LatestBankPair>>,
    mut resell_receiver: Option<mpsc::Receiver<SerializedSchedulerUpdate>>,
    mut mev_receiver: Option<mpsc::Receiver<SerializedSchedulerUpdate>>,
    mut tpu_receiver: Option<mpsc::Receiver<SerializedTpuUpdate>>,
    task_exit: Arc<AtomicBool>,
    shared_tin_log_epoch: Arc<AtomicU64>,
    exit: Arc<AtomicBool>,
) {
    let mut logged_connect_failure = false;
    let mut local_tin_log_epoch = shared_tin_log_epoch.load(Ordering::Relaxed);
    while !exit.load(Ordering::Relaxed) && !task_exit.load(Ordering::Relaxed) {
        match connect_maybe_autoconfig(
            &url,
            &uuid,
            resell,
            mev,
            enable_tpu_p2c_update,
            &cluster_info,
            &shared_bank_update,
            &mut resell_receiver,
            &mut mev_receiver,
            &mut tpu_receiver,
            &task_exit,
            &exit,
            &shared_tin_log_epoch,
            &mut local_tin_log_epoch,
        )
        .await
        {
            Ok(()) => {
                logged_connect_failure = false;
            }
            Err(error) => {
                error!("SchedulerUpdateNotifier: connection failed url={url} uuid={uuid}: {error}");
                if !logged_connect_failure {
                    tin_connection_state_log(
                        url.clone(),
                        String::new(),
                        uuid.clone(),
                        false,
                        format!("disconnected:error={error}"),
                    );
                    logged_connect_failure = true;
                }
                time::sleep(Duration::from_secs(30)).await;
            }
        }
    }
}

#[derive(Debug)]
enum NotifierError {
    Auth(String),
    Connect(String),
    Stream(String),
}

impl std::fmt::Display for NotifierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotifierError::Auth(msg) => write!(f, "auth error: {msg}"),
            NotifierError::Connect(msg) => write!(f, "connect error: {msg}"),
            NotifierError::Stream(msg) => write!(f, "stream error: {msg}"),
        }
    }
}

async fn connect_maybe_autoconfig(
    seed_url: &str,
    uuid: &str,
    resell: bool,
    mev: bool,
    enable_tpu_p2c_update: bool,
    cluster_info: &Arc<ClusterInfo>,
    shared_bank_update: &Arc<RwLock<LatestBankPair>>,
    resell_receiver: &mut Option<mpsc::Receiver<SerializedSchedulerUpdate>>,
    mev_receiver: &mut Option<mpsc::Receiver<SerializedSchedulerUpdate>>,
    tpu_receiver: &mut Option<mpsc::Receiver<SerializedTpuUpdate>>,
    task_exit: &Arc<AtomicBool>,
    exit: &Arc<AtomicBool>,
    shared_tin_log_epoch: &Arc<AtomicU64>,
    local_tin_log_epoch: &mut u64,
) -> Result<(), NotifierError> {
    let seed_endpoint =
        get_endpoint(seed_url).map_err(|error| NotifierError::Connect(error.to_string()))?;
    let candidates = autoconfig_ranked_candidates(seed_url, &seed_endpoint, P2C_AUTOCONFIG_METRICS)
        .await
        .map_err(|error| NotifierError::Connect(error.to_string()))?;

    let endpoint_count = candidates.len();
    datapoint_info!(
        P2C_AUTOCONFIG_METRICS.autoconfig,
        "type" => "connect",
        ("candidate_count", endpoint_count, i64),
        ("count", 1, i64),
    );
    for (p2c_url, p2c_endpoint, _shredstream_socket, latency_us) in candidates {
        info!(
            "SchedulerUpdateNotifier: trying P2C url={p2c_url} uuid={uuid} rtt=({:?})",
            Duration::from_micros(latency_us)
        );
        let connect_start = Instant::now();
        match auth_and_connect(
            seed_url,
            &p2c_url,
            &p2c_endpoint,
            uuid,
            resell,
            mev,
            enable_tpu_p2c_update,
            cluster_info,
            shared_bank_update,
            resell_receiver,
            mev_receiver,
            tpu_receiver,
            task_exit,
            exit,
            shared_tin_log_epoch,
            local_tin_log_epoch,
        )
        .await
        {
            Ok(()) => {
                datapoint_info!(
                    P2C_AUTOCONFIG_METRICS.autoconfig,
                    "type" => "closed_connection",
                    ("url", p2c_url, String),
                    ("count", 1, i64),
                );
                return Ok(());
            }
            Err(error) => {
                datapoint_warn!(
                    P2C_AUTOCONFIG_METRICS.error,
                    "type" => "proxy_err",
                    ("url", p2c_url, String),
                    ("count", 1, i64),
                    ("error", error.to_string(), String),
                );
                if connect_start.elapsed() > AUTH_CONNECTION_TIMEOUT * 3 {
                    return Err(error);
                }
            }
        }
    }
    Err(NotifierError::Connect(format!(
        "autoconfig failed: all {endpoint_count} candidate endpoints failed to connect"
    )))
}

async fn auth_and_connect(
    url: &str,
    actual_url: &str,
    p2c_endpoint: &Endpoint,
    uuid: &str,
    resell: bool,
    mev: bool,
    enable_tpu_p2c_update: bool,
    cluster_info: &Arc<ClusterInfo>,
    shared_bank_update: &Arc<RwLock<LatestBankPair>>,
    resell_receiver: &mut Option<mpsc::Receiver<SerializedSchedulerUpdate>>,
    mev_receiver: &mut Option<mpsc::Receiver<SerializedSchedulerUpdate>>,
    tpu_receiver: &mut Option<mpsc::Receiver<SerializedTpuUpdate>>,
    task_exit: &Arc<AtomicBool>,
    exit: &Arc<AtomicBool>,
    shared_tin_log_epoch: &Arc<AtomicU64>,
    local_tin_log_epoch: &mut u64,
) -> Result<(), NotifierError> {
    let keypair = cluster_info.keypair();

    let channel = time::timeout(AUTH_CONNECTION_TIMEOUT, p2c_endpoint.connect())
        .await
        .map_err(|_| NotifierError::Connect("connection timeout".to_string()))?
        .map_err(|error| NotifierError::Connect(error.to_string()))?;

    let mut auth_client = AuthServiceClient::new(channel.clone());
    let (access_token, refresh_token) = time::timeout(
        AUTH_CONNECTION_TIMEOUT,
        generate_auth_tokens_relayer(&mut auth_client, keypair.as_ref()),
    )
    .await
    .map_err(|_| NotifierError::Auth("auth timeout".to_string()))?
    .map_err(|error| NotifierError::Auth(error))?;

    let shared_access_token = Arc::new(arc_swap::ArcSwap::from_pointee(access_token));
    let auth_interceptor = P2cAuthInterceptor::new(shared_access_token.clone());

    let p2c_channel = time::timeout(AUTH_CONNECTION_TIMEOUT, p2c_endpoint.connect())
        .await
        .map_err(|_| NotifierError::Connect("p2c connection timeout".to_string()))?
        .map_err(|error| NotifierError::Connect(error.to_string()))?;

    let mut p2c_client: BlockEngineRelayerClient<InterceptedService<Channel, P2cAuthInterceptor>> =
        BlockEngineRelayerClient::with_interceptor(p2c_channel, auth_interceptor);

    // One TLS/auth connection multiplexes optional streams:
    // - resell → StartExpiringPacketStream (resell hash, no TPU)
    // - mev → StartExpiringMevPacketStream (+ optional TPU)
    let resell_grpc_tx = if resell {
        let (tx, rx) = mpsc::channel(GRPC_QUEUE_CAPACITY);
        p2c_client
            .start_expiring_packet_stream(ReceiverStream::new(rx))
            .await
            .map_err(|error: tonic::Status| {
                NotifierError::Stream(format!(
                    "StartExpiringPacketStream required when resell enabled: {error}"
                ))
            })?;
        Some(tx)
    } else {
        None
    };

    let mev_grpc_tx = if mev {
        let (tx, rx) = mpsc::channel(GRPC_QUEUE_CAPACITY);
        p2c_client
            .start_expiring_mev_packet_stream(ReceiverStream::new(rx))
            .await
            .map_err(|error: tonic::Status| {
                NotifierError::Stream(format!(
                    "StartExpiringMevPacketStream required when mev enabled: {error}"
                ))
            })?;
        Some(tx)
    } else {
        None
    };

    // Optional: StartExpiringTpuPacketStream — Mev only when flag is set.
    // Older Mev consumers may still return UNIMPLEMENTED; continue with Mev scheduler only.
    let tpu_grpc_tx = if tpu_receiver.is_some() && mev && enable_tpu_p2c_update {
        let (tpu_grpc_tx, tpu_grpc_rx) = mpsc::channel(GRPC_QUEUE_CAPACITY);
        match p2c_client
            .start_expiring_tpu_packet_stream(ReceiverStream::new(tpu_grpc_rx))
            .await
        {
            Ok(_tpu_response) => Some(tpu_grpc_tx),
            Err(error) => {
                warn!(
                    "SchedulerUpdateNotifier: StartExpiringTpuPacketStream unavailable \
                     url={url} actual_url={actual_url} uuid={uuid}: {error}; \
                     continuing with scheduler stream(s) only"
                );
                None
            }
        }
    } else {
        None
    };

    // Optional: older consumers may not implement StartP2cUpdateCountStream.
    let count_grpc_tx = {
        let (count_grpc_tx, count_grpc_rx) = mpsc::channel(GRPC_QUEUE_CAPACITY);
        match p2c_client
            .start_p2c_update_count_stream(ReceiverStream::new(count_grpc_rx))
            .await
        {
            Ok(_count_response) => Some(count_grpc_tx),
            Err(error) => {
                warn!(
                    "SchedulerUpdateNotifier: StartP2cUpdateCountStream unavailable \
                     url={url} actual_url={actual_url} uuid={uuid}: {error}; \
                     continuing without count gRPC (metrics dump still enabled)"
                );
                None
            }
        }
    };

    tin_connection_state_log(
        url.to_string(),
        actual_url.to_string(),
        uuid.to_string(),
        false,
        "connected".to_string(),
    );
    match handle_postpack_confirmation_stream(
        resell_grpc_tx,
        mev_grpc_tx,
        tpu_grpc_tx,
        count_grpc_tx,
        enable_tpu_p2c_update,
        resell_receiver,
        mev_receiver,
        tpu_receiver,
        auth_client,
        cluster_info,
        shared_bank_update,
        refresh_token,
        shared_access_token,
        task_exit,
        exit,
        url,
        actual_url,
        uuid,
        shared_tin_log_epoch,
        local_tin_log_epoch,
    )
    .await
    {
        Ok(()) => {
            tin_connection_state_log(
                url.to_string(),
                actual_url.to_string(),
                uuid.to_string(),
                false,
                "disconnected".to_string(),
            );
            Ok(())
        }
        Err(error) => {
            error!(
                "SchedulerUpdateNotifier: stream ended with error url={url} \
                 actual_url={actual_url} uuid={uuid}: {error}"
            );
            tin_connection_state_log(
                url.to_string(),
                actual_url.to_string(),
                uuid.to_string(),
                false,
                format!("disconnected:error={error}"),
            );
            Err(error)
        }
    }
}

async fn handle_postpack_confirmation_stream(
    resell_grpc_sender: Option<mpsc::Sender<PacketBatchUpdate>>,
    mev_grpc_sender: Option<mpsc::Sender<PacketBatchUpdate>>,
    tpu_grpc_sender: Option<mpsc::Sender<PacketBatchUpdate>>,
    count_grpc_sender: Option<mpsc::Sender<P2cUpdateCount>>,
    enable_tpu_p2c_update: bool,
    resell_receiver: &mut Option<mpsc::Receiver<SerializedSchedulerUpdate>>,
    mev_receiver: &mut Option<mpsc::Receiver<SerializedSchedulerUpdate>>,
    tpu_receiver: &mut Option<mpsc::Receiver<SerializedTpuUpdate>>,
    mut auth_client: AuthServiceClient<Channel>,
    cluster_info: &Arc<ClusterInfo>,
    shared_bank_update: &Arc<RwLock<LatestBankPair>>,
    mut refresh_token: Token,
    shared_access_token: Arc<arc_swap::ArcSwap<Token>>,
    task_exit: &Arc<AtomicBool>,
    exit: &Arc<AtomicBool>,
    url: &str,
    actual_url: &str,
    uuid: &str,
    shared_tin_log_epoch: &Arc<AtomicU64>,
    local_tin_log_epoch: &mut u64,
) -> Result<(), NotifierError> {
    if let Some(rx) = resell_receiver.as_mut() {
        while rx.try_recv().is_ok() {}
    }
    if let Some(rx) = mev_receiver.as_mut() {
        while rx.try_recv().is_ok() {}
    }
    if let Some(rx) = tpu_receiver.as_mut() {
        while rx.try_recv().is_ok() {}
    }

    let mut heartbeat_interval = interval(HEARTBEAT_INTERVAL);
    let mut slot_poll_interval = interval(SLOT_POLL_INTERVAL);
    let mut auth_refresh_interval = interval(AUTH_REFRESH_INTERVAL);
    let mut heartbeat_count = 0u64;
    let mut count_tracker =
        P2cUpdateCountTracker::new(uuid.to_string(), enable_tpu_p2c_update, count_grpc_sender);
    // Seed current slot so idle connected slots still get dumped on roll.
    count_tracker.poll_slot(shared_bank_update).await;

    let result = async {
        while !exit.load(Ordering::Relaxed) && !task_exit.load(Ordering::Relaxed) {
            tokio::select! {
                _ = heartbeat_interval.tick() => {
                    maybe_log_tin_connection_for_epoch(
                        shared_tin_log_epoch,
                        local_tin_log_epoch,
                        url,
                        actual_url,
                        uuid,
                    );
                    let heartbeat_msg = || PacketBatchUpdate {
                        msg: Some(Msg::Heartbeat(Heartbeat { count: heartbeat_count })),
                    };
                    if let Some(sender) = &resell_grpc_sender {
                        sender
                            .send(heartbeat_msg())
                            .await
                            .map_err(|error| NotifierError::Stream(error.to_string()))?;
                    }
                    if let Some(sender) = &mev_grpc_sender {
                        sender
                            .send(heartbeat_msg())
                            .await
                            .map_err(|error| NotifierError::Stream(error.to_string()))?;
                    }
                    if let Some(tpu_grpc_sender) = &tpu_grpc_sender {
                        tpu_grpc_sender
                            .send(heartbeat_msg())
                            .await
                            .map_err(|error| NotifierError::Stream(error.to_string()))?;
                    }
                    heartbeat_count = heartbeat_count.saturating_add(1);
                }

                _ = slot_poll_interval.tick() => {
                    count_tracker.poll_slot(shared_bank_update).await;
                }

                maybe_update = async {
                    match resell_receiver.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let Some(update) = maybe_update else {
                        return Err(NotifierError::Stream(
                            "resell update receiver disconnected".to_string(),
                        ));
                    };
                    let Some(resell_grpc_sender) = &resell_grpc_sender else {
                        continue;
                    };
                    let slot = P2cUpdateCountTracker::working_slot(shared_bank_update)
                        .unwrap_or(0) as u32;
                    forward_packet_batch_update(
                        resell_grpc_sender,
                        &update.0,
                        update.1.to_string(),
                        slot,
                    )
                    .await?;
                    count_tracker.on_scheduler_sent(shared_bank_update).await;
                }

                maybe_update = async {
                    match mev_receiver.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let Some(update) = maybe_update else {
                        return Err(NotifierError::Stream(
                            "mev update receiver disconnected".to_string(),
                        ));
                    };
                    let Some(mev_grpc_sender) = &mev_grpc_sender else {
                        continue;
                    };
                    let slot = P2cUpdateCountTracker::working_slot(shared_bank_update)
                        .unwrap_or(0) as u32;
                    forward_packet_batch_update(
                        mev_grpc_sender,
                        &update.0,
                        update.1.to_string(),
                        slot,
                    )
                    .await?;
                    count_tracker.on_scheduler_sent(shared_bank_update).await;
                }

                maybe_tpu_update = async {
                    match tpu_receiver.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let Some(update) = maybe_tpu_update else {
                        return Err(NotifierError::Stream(
                            "TPU update receiver disconnected".to_string(),
                        ));
                    };
                    let Some(tpu_grpc_sender) = &tpu_grpc_sender else {
                        continue;
                    };
                    let slot = P2cUpdateCountTracker::working_slot(shared_bank_update)
                        .unwrap_or(0) as u32;
                    forward_packet_batch_update(
                        tpu_grpc_sender,
                        &update.0,
                        update.1.to_string(),
                        slot,
                    )
                    .await?;
                    count_tracker.on_tpu_sent(shared_bank_update).await;
                }

                _ = auth_refresh_interval.tick() => {
                    match maybe_refresh_relayer_auth(
                        &mut auth_client,
                        keypair_from_cluster(cluster_info),
                        &shared_access_token,
                        &refresh_token,
                    )
                    .await
                    {
                        Ok(Some(new_refresh_token)) => refresh_token = new_refresh_token,
                        Ok(None) => {}
                        Err(error) => {
                            warn!("SchedulerUpdateNotifier: auth refresh failed: {error}");
                            return Err(NotifierError::Auth(error));
                        }
                    }
                }
            }
        }
        Ok(())
    }
    .await;

    // Flush in-progress slot counts on clean exit or stream error.
    count_tracker.flush().await;
    result
}

async fn forward_packet_batch_update(
    grpc_sender: &mpsc::Sender<PacketBatchUpdate>,
    byte_packet: &BytesPacket,
    addr: String,
    slot: u32,
) -> Result<(), NotifierError> {
    let packet = ProtoPacket {
        data: byte_packet.buffer().to_vec().into(),
        meta: Some(ProtoMeta {
            size: byte_packet.buffer().len() as u64,
            addr,
            port: 0,
            flags: None,
            sender_stake: 0,
        }),
    };

    grpc_sender
        .send(PacketBatchUpdate {
            msg: Some(Msg::Batches(ExpiringPacketBatch {
                header: Some(Header {
                    ts: Some(Timestamp::from(SystemTime::now())),
                }),
                batch: Some(ProtoPacketBatch {
                    packets: vec![packet],
                }),
                // Wire field is expiry_ms; we carry the working-bank slot as u32.
                expiry_ms: slot,
            })),
        })
        .await
        .map_err(|error| NotifierError::Stream(error.to_string()))
}

fn keypair_from_cluster(cluster_info: &Arc<ClusterInfo>) -> Arc<solana_keypair::Keypair> {
    cluster_info.keypair().clone()
}

async fn maybe_refresh_relayer_auth(
    auth_client: &mut AuthServiceClient<Channel>,
    keypair: Arc<solana_keypair::Keypair>,
    shared_access_token: &Arc<arc_swap::ArcSwap<Token>>,
    refresh_token: &Token,
) -> Result<Option<Token>, String> {
    use p2c_protos::pre_conf::auth::RefreshAccessTokenRequest;

    let access_token = shared_access_token.load();
    let access_token_expiration = access_token
        .expires_at_utc
        .as_ref()
        .ok_or_else(|| "missing access token expiration".to_string())?;
    let access_token_expiration =
        SystemTime::try_from(access_token_expiration.clone()).map_err(|error| error.to_string())?;
    let access_token_duration_left = access_token_expiration.duration_since(SystemTime::now());

    let refresh_token_expiration = refresh_token
        .expires_at_utc
        .as_ref()
        .ok_or_else(|| "missing refresh token expiration".to_string())?;
    let refresh_token_expiration = SystemTime::try_from(refresh_token_expiration.clone())
        .map_err(|error| error.to_string())?;
    let refresh_token_duration_left = refresh_token_expiration.duration_since(SystemTime::now());

    let is_access_token_expiring_soon = match access_token_duration_left {
        Ok(duration) => duration < Duration::from_secs(AUTH_REFRESH_WITHIN_S),
        Err(_) => true,
    };
    let is_refresh_token_expiring_soon = match refresh_token_duration_left {
        Ok(duration) => duration < Duration::from_secs(AUTH_REFRESH_WITHIN_S),
        Err(_) => true,
    };

    match (
        is_refresh_token_expiring_soon,
        is_access_token_expiring_soon,
    ) {
        (true, _) => {
            let (access_token, new_refresh_token) =
                generate_auth_tokens_relayer(auth_client, keypair.as_ref()).await?;
            shared_access_token.store(Arc::new(access_token));
            Ok(Some(new_refresh_token))
        }
        (false, true) => {
            let response = auth_client
                .refresh_access_token(RefreshAccessTokenRequest {
                    refresh_token: refresh_token.value.clone(),
                })
                .await
                .map_err(|error| error.to_string())?;

            let access_token = response
                .into_inner()
                .access_token
                .ok_or_else(|| "missing access token".to_string())?;
            shared_access_token.store(Arc::new(access_token));
            Ok(None)
        }
        (false, false) => Ok(None),
    }
}

async fn generate_auth_tokens_relayer(
    auth_client: &mut AuthServiceClient<Channel>,
    keypair: &solana_keypair::Keypair,
) -> Result<(Token, Token), String> {
    use {
        p2c_protos::pre_conf::auth::{
            GenerateAuthChallengeRequest, GenerateAuthTokensRequest, GenerateAuthTokensResponse,
            Role,
        },
        solana_signer::Signer,
    };

    let auth_response = auth_client
        .generate_auth_challenge(GenerateAuthChallengeRequest {
            role: Role::Relayer.into(),
            pubkey: keypair.pubkey().to_bytes().to_vec(),
        })
        .await
        .map_err(|error| error.to_string())?;

    let challenge = format!(
        "{}-{}",
        keypair.pubkey(),
        auth_response.into_inner().challenge
    );
    let signed_challenge = keypair.sign_message(challenge.as_bytes()).as_ref().to_vec();

    let GenerateAuthTokensResponse {
        access_token: maybe_access_token,
        refresh_token: maybe_refresh_token,
    } = auth_client
        .generate_auth_tokens(GenerateAuthTokensRequest {
            challenge,
            client_pubkey: keypair.pubkey().as_ref().to_vec(),
            signed_challenge,
        })
        .await
        .map_err(|error| error.to_string())?
        .into_inner();

    let access_token = maybe_access_token.ok_or_else(|| "missing access token".to_string())?;
    let refresh_token = maybe_refresh_token.ok_or_else(|| "missing refresh token".to_string())?;

    if access_token.expires_at_utc.is_none() || refresh_token.expires_at_utc.is_none() {
        return Err("auth tokens missing expiration".to_string());
    }

    Ok((access_token, refresh_token))
}

fn blocklisted_entries_from_merged(
    merged_entries: &[PostPackConfirmation],
    blocklisted_uuids: &[String],
) -> Vec<PostPackConfirmation> {
    let blocklist: HashSet<&str> = blocklisted_uuids.iter().map(String::as_str).collect();
    merged_entries
        .iter()
        .filter(|entry| blocklist.contains(entry.uuid.as_str()))
        .cloned()
        .collect()
}

fn filter_blocklisted_uuids(
    entries: &[PostPackConfirmation],
    blocklisted_uuids: &[String],
) -> Vec<PostPackConfirmation> {
    let blocklist: HashSet<&str> = blocklisted_uuids.iter().map(String::as_str).collect();
    entries
        .iter()
        .filter(|entry| !blocklist.contains(entry.uuid.as_str()))
        .cloned()
        .collect()
}

fn union_postpack_confirmation_entries(
    pda_entries: &[PostPackConfirmation],
    admin_entries: &[PostPackConfirmation],
) -> Vec<PostPackConfirmation> {
    // One entry per UUID; admin overwrites on-chain on the same key.
    let mut by_uuid = HashMap::new();
    for entry in pda_entries {
        by_uuid.insert(entry.uuid.clone(), entry.clone());
    }
    for entry in admin_entries {
        by_uuid.insert(entry.uuid.clone(), entry.clone());
    }
    by_uuid.into_values().collect()
}
