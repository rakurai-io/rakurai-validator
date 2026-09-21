use {
    crate::{
        admin_rpc_post_init::{KeyUpdaterType, KeyUpdaters},
        gui::{
            gui_snapshot::{
                connect_snapshot_messages, format_epoch_messages, format_slot_update_message,
                format_query_rankings_response_from_store, new_snapshot_channel,
                GuiIdentityUpdater, GuiWsSnapshotRequest,
            },
            metrics::{
                format_live_txn_waterfall_message, live_txn_waterfall_for_send, RetainedSnapshot,
                TxnWaterfall,
            },
            slot_query::{
                format_query_transactions_response, new_query_channel, new_rankings_channel,
                GuiWsQuery, GuiWsRankingsQuery,
            },
            slot_store::SlotTxnStore,
            ws_server::{new_broadcast_sender, serve, GuiIpWhitelist},
        },
        proxy::block_engine_stage::BlockEngineStageStats,
        bundle_sigverify_stage::BundleSigverifyStageStats,
        bundle_stage::BundleStageLoopMetrics,
        banking_stage::transaction_scheduler::scheduler_metrics::SchedulerCountMetricsInner,
        banking_stage::consume_worker::ConsumeWorkerCountMetrics,
    },
    crossbeam_channel::Receiver,
    solana_clock::Slot,
    solana_poh::poh_recorder::SharedLeaderState,
    solana_svm_timings::wallclock_timestamp_nanos,
    solana_streamer::quic::GuiStreamerMetrics,
    std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, RwLock,
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    },
};

pub mod gui_snapshot;
pub mod metrics;
pub mod slot_query;
pub mod slot_store;
pub mod slot_txn;
pub mod txn_report;
mod ws_server;

pub use gui_snapshot::GuiContext;
pub use slot_txn::GuiTxnEvent;

/// GUI Banks-panel row for simple votes.
pub const GUI_TXN_BANK_IDX_VOTE: u8 = 0;
/// GUI Banks-panel row for bundle transactions.
pub const GUI_TXN_BANK_IDX_BUNDLE: u8 = 1;
/// First GUI Banks-panel row for non-vote consume workers; worker `i` uses `NON_VOTE_BASE + i`.
pub const GUI_TXN_BANK_IDX_NON_VOTE_BASE: u8 = 2;

/// Map consume-worker thread index to GUI `bank_idx`.
#[inline(always)]
pub const fn gui_txn_bank_idx_for_worker(thread_index: usize) -> u8 {
    GUI_TXN_BANK_IDX_NON_VOTE_BASE.wrapping_add(thread_index as u8)
}

/// Total GUI bank tiles: vote + bundle + one per consume worker.
#[inline(always)]
pub const fn gui_bank_tile_count(num_consume_workers: usize) -> usize {
    GUI_TXN_BANK_IDX_NON_VOTE_BASE as usize + num_consume_workers
}

/// Per-transaction schedule metadata captured when a batch is sent to a consume worker.
/// Parallel to `ConsumeWork::transactions`; populated by the Rakurai scheduler for GUI metrics.
#[derive(Clone, Debug, Default)]
pub struct GuiTxnScheduleInfo {
    pub bank_idx: u8,
    /// Wall-clock nanos when the scheduler sent this batch (GUI microblock start).
    pub microblock_start_timestamp_nanos: i64,
    /// Wall-clock nanos when the packet was first received (0 if unset).
    pub timestamp_arrival_nanos: i64,
    /// Source IPv4 of the packet as a big-endian `u32` (0 if unset/non-IPv4).
    pub source_ipv4: u32,
    /// Ingress transport for this transaction (defaults to `Quic`).
    pub source_tpu: metrics::GuiTxnTpuSource,
}

const GUI_UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const GUI_LOOP_INTERVAL: Duration = Duration::from_millis(10);
const GUI_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);
#[repr(C)]
pub struct SchedulerReceptionMetrics {
    pub num_received: usize,
    pub num_dropped_without_parsing: usize,
    pub num_buffered: usize,
    pub num_sent: usize,
    pub num_sent_high_priority: usize,
    pub num_too_old: usize,
    pub num_dropped_on_sanitization: usize,
    pub num_dropped_on_validate_locks: usize,
    pub num_dropped_on_capacity: usize,
    pub num_dropped_on_cleanup: usize,
    pub num_dropped_on_already_processed: usize,
    pub num_dropped_on_fee_payer: usize,
    pub num_dropped_on_age: usize,
    pub min_prioritization_fees: u64,
    pub max_prioritization_fees: u64,
    pub sender_throttling_count: usize,
    pub num_dropped_on_blacklisted_account: usize,
    pub num_dropped_on_compute_budget: usize,
}

#[repr(C)]
pub struct SchedulerMetrics {
    pub num_received: usize,
    pub num_buffered: usize,
    pub num_scheduled: usize,
    pub num_unschedulable: usize,
    pub num_schedule_filtered_out: usize,
    pub num_finished: usize,
    pub num_retryable: usize,
    pub num_forwarded: usize,
    pub num_dropped_on_receive: usize,
    pub num_dropped_on_sanitization: usize,
    pub num_dropped_on_validate_locks: usize,
    pub num_dropped_on_receive_transaction_checks: usize,
    pub num_dropped_on_clear: usize,
    pub num_dropped_on_age_and_status: usize,
    pub num_dropped_on_capacity: usize,
    pub min_prioritization_fees: u64,
    pub max_prioritization_fees: u64,
    pub unscheduled_on_cu_limit: usize,
    pub num_consume_decision: usize,
    pub num_nonconsume_decision: usize,
    pub considered_tx_break: usize,
    pub num_cu_throttled: u64,
    pub num_acct_cu_throttled: u64,
    pub enabled_turns: u64,
    pub disabled_turns: u64,
    pub num_considered: u64,
    pub schedulable_threads_empty_count: u64,
    pub container_empty_count: u64,
    pub update_cost_tracker_count: u64,
}

#[repr(C)]
pub struct Scheduler2Metrics {
    pub num_received: usize,
    pub num_buffered: usize,
    pub num_scheduled: usize,
    pub num_unschedulable: usize,
    pub num_schedule_filtered_out: usize,
    pub num_finished: usize,
    pub num_retryable: usize,
    pub num_forwarded: usize,
    pub num_dropped_on_receive: usize,
    pub num_dropped_on_sanitization: usize,
    pub num_dropped_on_validate_locks: usize,
    pub num_dropped_on_receive_transaction_checks: usize,
    pub num_dropped_on_clear: usize,
    pub num_dropped_on_age_and_status: usize,
    pub num_dropped_on_capacity: usize,
    pub min_prioritization_fees: u64,
    pub max_prioritization_fees: u64,
    pub num_consume_decision: u64,
    pub empty_turns: u64,
    pub scheduler_enabled_count: u64,
    pub scheduler_disabled_count: u64,
    pub cu_limit_counter: u64,
    pub backlog_limit_counter: u64,
    pub top_accts_decision_count: u64,
    pub accts_lookup_decision_count: u64,
    pub load_balancing_decision_count: u64,
    pub acct_limit_reached_counter: u64,
    pub cu_throttled_counter: u64,
    pub nonce_filtered_out: u64,
}

pub struct GuiBankingStageStats {
    pub tpu_receive_and_buffer_packets_count: u64,
    pub tpu_dropped_packets_count: u64,
    pub gossip_receive_and_buffer_packets_count: u64,
    pub gossip_dropped_packets_count: u64,
    pub dropped_forward_gossip_packets_count: u64,
    pub dropped_forward_tpu_packets_count: u64,
}

pub struct GuiSigVerifierStats {
    pub total_packets: u64,
    pub total_dedup: u64,
    pub total_valid_packets: u64,
    pub eviction_drops: u64,
}

pub struct GuiVoteStats {
    pub newly_failed_sigverify_count: u64,
    pub failed_sanitization_count: u64,
    pub failed_prioritization_count: u64,
    pub invalid_votes_count: u64,
    pub retryable_packets_filtered_count: u64,
    pub committed_transactions_count: u64,
    pub committed_transactions_with_successful_result_count: u64,
    pub nonretryable_errored_transactions_count: u64,
    pub executed_transactions_failed_commit_count: u64,
}

#[repr(C)]
pub enum GuiCoreMetrics {
    BlockEngine(BlockEngineStageStats),
    BundleStage(BundleStageLoopMetrics),
    SchedReceptionPackRetained(u64),
    SchedPackRetained(u64),
    PackRetained(u64),
    SigVerify(GuiSigVerifierStats),
    BundleSigverify(BundleSigverifyStageStats),
    SchedulerReception(SchedulerReceptionMetrics),
    Scheduler(SchedulerMetrics),
    Scheduler2(Scheduler2Metrics),
    StandardScheduler(SchedulerCountMetricsInner),
    ConsumeWorker(ConsumeWorkerCountMetrics),
    BankingStageVote(GuiBankingStageStats),
    VoteWorker(GuiVoteStats),
    FetchStageForwardDropped(u64),
    FetchStageForwardDiscard(u64),
    FetchStageManagerForwardDropped(u64),
}
pub struct GuiPanels {
    pub live_txn_waterfall: TxnWaterfall,
    sched_reception_pack_retained: u64,
    sched_pack_retained: u64,
}

pub struct Gui {
    handle: Option<JoinHandle<()>>,
}

impl Gui {
    pub fn new(
        core_receiver: Option<Receiver<GuiCoreMetrics>>,
        streamer_receiver: Option<Receiver<GuiStreamerMetrics>>,
        txn_receiver: Option<Receiver<slot_txn::GuiTxnEvent>>,
        shared_leader_state: SharedLeaderState,
        gui_context: GuiContext,
        exit: Arc<AtomicBool>,
        listen_addr: &str,
        max_websocket_connections: usize,
        ip_whitelist: GuiIpWhitelist,
        key_notifiers: Option<Arc<RwLock<KeyUpdaters>>>,
    ) -> Self {
        let enabled = core_receiver.is_some()
            && streamer_receiver.is_some()
            && txn_receiver.is_some();

        if !enabled {
            return Self { handle: None };
        }

        let gui_panels = GuiPanels {
            live_txn_waterfall: TxnWaterfall::default(),
            sched_reception_pack_retained: 0,
            sched_pack_retained: 0,
        };
        let listen_addr = listen_addr.to_string();
        // Create the WS fanout before spawning so setIdentity can register a notifier
        // that broadcasts to the same channel live clients subscribe to.
        let ws_sender = new_broadcast_sender();
        if let Some(key_notifiers) = key_notifiers {
            key_notifiers.write().unwrap().add(
                KeyUpdaterType::Gui,
                Arc::new(GuiIdentityUpdater::new(
                    gui_context.identity.clone(),
                    ws_sender.clone(),
                )),
            );
        }
        let handle = match thread::Builder::new()
            .name("solGui".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        log::error!("failed to create gui runtime: {err}");
                        return;
                    }
                };

                runtime.block_on(async move {
                    let ws_sender_for_metrics = ws_sender.clone();
                    let (query_sender, query_receiver) = new_query_channel();
                    let (rankings_sender, rankings_receiver) = new_rankings_channel();
                    let (snapshot_sender, snapshot_receiver) = new_snapshot_channel();
                    tokio::join!(
                        serve(
                            &listen_addr,
                            exit.clone(),
                            ws_sender,
                            query_sender,
                            rankings_sender,
                            snapshot_sender,
                            gui_context.clone(),
                            max_websocket_connections,
                            ip_whitelist,
                        ),
                        Self::run(
                            core_receiver,
                            streamer_receiver,
                            txn_receiver,
                            shared_leader_state,
                            gui_context,
                            gui_panels,
                            ws_sender_for_metrics,
                            query_receiver,
                            rankings_receiver,
                            snapshot_receiver,
                            exit,
                        ),
                    );
                });
            }) {
            Ok(handle) => Some(handle),
            Err(err) => {
                log::error!("failed to spawn gui thread: {err}");
                None
            }
        };
        Self { handle }
    }

    pub fn join(self) -> thread::Result<()> {
        if let Some(handle) = self.handle {
            if handle.join().is_err() {
                log::error!("gui thread panicked during shutdown");
            }
        }
        Ok(())
    }

    async fn run(
        core_receiver: Option<Receiver<GuiCoreMetrics>>,
        streamer_receiver: Option<Receiver<GuiStreamerMetrics>>,
        txn_receiver: Option<Receiver<slot_txn::GuiTxnEvent>>,
        shared_leader_state: SharedLeaderState,
        gui_context: GuiContext,
        mut gui_panels: GuiPanels,
        ws_sender: tokio::sync::broadcast::Sender<String>,
        mut query_receiver: tokio::sync::mpsc::Receiver<GuiWsQuery>,
        mut rankings_receiver: tokio::sync::mpsc::Receiver<GuiWsRankingsQuery>,
        mut snapshot_receiver: tokio::sync::mpsc::Receiver<GuiWsSnapshotRequest>,
        exit: Arc<AtomicBool>,
    ) {
        let Some(core_receiver) = core_receiver else {
            return;
        };
        let Some(streamer_receiver) = streamer_receiver else {
            return;
        };
        let Some(txn_receiver) = txn_receiver else {
            return;
        };

        let mut retained_snapshot = RetainedSnapshot::default();
        let mut slot_txn_store = SlotTxnStore::new();
        let mut next_leader_slot: Option<Slot> = None;
        let mut last_leader_slot: Option<Slot> = None;
        let mut next_sample = Instant::now();
        let mut next_snapshot = Instant::now();
        let mut last_broadcast_epoch: Option<u64> = None;
        let mut last_broadcast_slot: Option<Slot> = None;

        Self::broadcast_connect_snapshot(&gui_context, &ws_sender);

        log::info!("gui metrics loop started");
        while !exit.load(Ordering::Relaxed) {

            while let Ok(metrics) = streamer_receiver.try_recv() {
                Self::apply_streamer_metrics(&mut gui_panels, metrics);
            }

            while let Ok(metrics) = core_receiver.try_recv() {
                Self::apply_core_metrics(&mut gui_panels, metrics);
            }

            while let Ok(event) = txn_receiver.try_recv() {
                slot_txn_store.handle_event(event);
            }

            while let Ok(query) = query_receiver.try_recv() {
                Self::handle_ws_query(
                    &slot_txn_store,
                    &shared_leader_state,
                    query,
                );
            }

            while let Ok(request) = rankings_receiver.try_recv() {
                if let Some(response) =
                    format_query_rankings_response_from_store(&slot_txn_store, request.id)
                {
                    let _ = request.reply.send(response);
                }
            }

            while let Ok(request) = snapshot_receiver.try_recv() {
                let _ = request
                    .reply
                    .send(connect_snapshot_messages(&gui_context));
            }

            if Instant::now() >= next_snapshot {
                Self::maybe_broadcast_epoch(&gui_context, &ws_sender, &mut last_broadcast_epoch);
                Self::maybe_broadcast_slot_update(
                    &gui_context,
                    &ws_sender,
                    &mut last_broadcast_slot,
                );
                next_snapshot += GUI_SNAPSHOT_INTERVAL;
            }

            if Instant::now() >= next_sample {
                let waterfall = live_txn_waterfall_for_send(
                    &gui_panels.live_txn_waterfall,
                    &retained_snapshot,
                );
                if let Some(payload) =
                    format_live_txn_waterfall_message(next_leader_slot, &waterfall)
                {
                    let _ = ws_sender.send(payload);
                }
                next_sample += GUI_UPDATE_INTERVAL;
            }

            Self::update_leader_slot_state(
                &shared_leader_state,
                &mut last_leader_slot,
                &mut next_leader_slot,
                &mut retained_snapshot,
                &mut gui_panels,
                &mut slot_txn_store,
            );

            tokio::time::sleep(GUI_LOOP_INTERVAL).await;
        }
        log::info!("gui metrics loop stopped");
    }

    fn handle_ws_query(
        slot_txn_store: &SlotTxnStore,
        shared_leader_state: &SharedLeaderState,
        query: GuiWsQuery,
    ) {
        let active_leader_slot = shared_leader_state
            .load()
            .working_bank()
            .map(|bank| bank.slot());
        let is_active_leader = active_leader_slot == Some(query.slot);
        let history = slot_txn_store.get_queryable(query.slot, active_leader_slot);
        let Some(response) = format_query_transactions_response(
            query.slot,
            query.id,
            history,
            is_active_leader,
        ) else {
            log::warn!(
                "failed to serialize slot.query_transactions response for slot {}",
                query.slot
            );
            return;
        };
        let _ = query.reply.send(response);
    }

    fn broadcast_connect_snapshot(
        gui_context: &GuiContext,
        ws_sender: &tokio::sync::broadcast::Sender<String>,
    ) {
        for message in connect_snapshot_messages(gui_context) {
            let _ = ws_sender.send(message);
        }
    }

    fn maybe_broadcast_epoch(
        gui_context: &GuiContext,
        ws_sender: &tokio::sync::broadcast::Sender<String>,
        last_broadcast_epoch: &mut Option<u64>,
    ) {
        let epoch = {
            let Ok(bank_forks) = gui_context.bank_forks.read() else {
                return;
            };
            bank_forks.working_bank().epoch()
        };
        if last_broadcast_epoch == &Some(epoch) {
            return;
        }
        let messages = format_epoch_messages(gui_context);
        if messages.is_empty() {
            return;
        }
        *last_broadcast_epoch = Some(epoch);
        for message in messages {
            let _ = ws_sender.send(message);
        }
    }

    fn maybe_broadcast_slot_update(
        gui_context: &GuiContext,
        ws_sender: &tokio::sync::broadcast::Sender<String>,
        last_broadcast_slot: &mut Option<Slot>,
    ) {
        let bank_forks = match gui_context.bank_forks.read() {
            Ok(bank_forks) => bank_forks,
            Err(_) => return,
        };
        let slot = bank_forks.working_bank().slot();
        if slot == 0 || last_broadcast_slot == &Some(slot) {
            return;
        }
        *last_broadcast_slot = Some(slot);
        if let Some(message) = format_slot_update_message(gui_context) {
            let _ = ws_sender.send(message);
        }
    }

    fn apply_streamer_metrics(gui_panels: &mut GuiPanels, metrics: GuiStreamerMetrics) {
        match metrics {
            GuiStreamerMetrics::Quic(stats) => {
            gui_panels.live_txn_waterfall.in_.quic = gui_panels
                .live_txn_waterfall
                .in_
                .quic
                .saturating_add(
                    stats.total_packets_sent_to_consumer
                    + stats.total_handle_chunk_to_packet_send_err
                    + stats.total_packet_batches_none
                    + stats.invalid_stream_size
                    + stats.total_stream_read_errors
                    + stats.total_stream_read_timeouts
                );

            gui_panels.live_txn_waterfall.out.quic_overrun = gui_panels
                .live_txn_waterfall
                .out
                .quic_overrun
                .saturating_add(stats.total_handle_chunk_to_packet_send_full_err);
            
            gui_panels.live_txn_waterfall.out.quic_abandoned = gui_panels
                .live_txn_waterfall
                .out
                .quic_abandoned
                .saturating_add(stats.total_stream_read_timeouts);

            gui_panels.live_txn_waterfall.out.tpu_quic_invalid = gui_panels
                .live_txn_waterfall
                .out
                .tpu_quic_invalid
                .saturating_add(
                    stats.total_handle_chunk_to_packet_send_disconnected_err
                    + stats.total_packet_batches_none
                    + stats.invalid_stream_size
                    + stats.total_stream_read_errors
                );
        }
        GuiStreamerMetrics::Udp(stats) => {
            gui_panels.live_txn_waterfall.in_.udp = gui_panels
                .live_txn_waterfall
                .in_
                .udp
                .saturating_add(stats.packets_count as u64);

            gui_panels.live_txn_waterfall.out.tpu_udp_invalid = gui_panels
                .live_txn_waterfall
                .out
                .tpu_udp_invalid
                .saturating_add(stats.num_packets_dropped as u64);
        }
    }
    }

    fn sync_pack_retained(gui_panels: &mut GuiPanels) {
        gui_panels.live_txn_waterfall.out.pack_retained = gui_panels
            .sched_reception_pack_retained
            .saturating_add(gui_panels.sched_pack_retained);
    }

    fn apply_core_metrics(gui_panels: &mut GuiPanels, metrics: GuiCoreMetrics) {
        match metrics {
            GuiCoreMetrics::BlockEngine(stats) => {
                gui_panels.live_txn_waterfall.in_.block_engine = gui_panels
                    .live_txn_waterfall
                    .in_
                    .block_engine
                    .saturating_add(stats.num_bundles);
            }
            GuiCoreMetrics::SchedReceptionPackRetained(count) => {
                gui_panels.sched_reception_pack_retained = count;
                Self::sync_pack_retained(gui_panels);
            }
            GuiCoreMetrics::SchedPackRetained(count) => {
                gui_panels.sched_pack_retained = count;
                Self::sync_pack_retained(gui_panels);
            }
            GuiCoreMetrics::PackRetained(count) => {
                gui_panels.live_txn_waterfall.out.pack_retained = count;
            }
            GuiCoreMetrics::BundleStage(stats) => {
                // gui_panels.live_txn_waterfall.in_.pack_cranked = gui_panels
                //     .live_txn_waterfall
                //     .in_
                //     .pack_cranked
                //     .saturating_add(stats.tip_programs_error.0);

                gui_panels.live_txn_waterfall.out.dedup_duplicate = gui_panels
                    .live_txn_waterfall
                    .out
                    .dedup_duplicate
                    .saturating_add(stats.num_bundles_dropped_duplicate_transaction.0);

                gui_panels.live_txn_waterfall.out.pack_invalid_bundle = gui_panels
                    .live_txn_waterfall
                    .out
                    .pack_invalid_bundle
                    .saturating_add((stats.num_bundles_dropped.0 - stats.num_bundles_dropped_duplicate_transaction.0) as u64);
            }
            GuiCoreMetrics::SigVerify(stats) => {
                let verify_parse_delta = (stats.total_packets as i64)
                .wrapping_sub(stats.total_valid_packets as i64)
                .wrapping_sub(stats.total_dedup as i64)
                .wrapping_add(stats.eviction_drops as i64);
                gui_panels.live_txn_waterfall.out.verify_parse = gui_panels
                    .live_txn_waterfall
                    .out
                    .verify_parse
                    .saturating_add(verify_parse_delta.max(0) as u64);

                gui_panels.live_txn_waterfall.out.verify_duplicate = gui_panels
                    .live_txn_waterfall
                    .out
                    .verify_duplicate
                    .saturating_add(stats.total_dedup as u64);
            }
            GuiCoreMetrics::BundleSigverify(stats) => {
                gui_panels.live_txn_waterfall.out.verify_failed = gui_panels
                    .live_txn_waterfall
                    .out
                    .verify_failed
                    .saturating_add((stats.num_packets_failed_sigverify + stats.num_packets_failed_send) as u64);
            }
            GuiCoreMetrics::SchedulerReception(stats) => {
                // gui_panels.live_txn_waterfall.out.pack_expired = gui_panels
                //     .live_txn_waterfall
                //     .out
                //     .pack_expired
                //     .saturating_add(stats.num_dropped_without_parsing as u64);

                gui_panels.live_txn_waterfall.out.resolv_lut_failed = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_lut_failed
                    .saturating_add(stats.num_dropped_on_sanitization as u64);

                gui_panels.live_txn_waterfall.out.resolv_expired = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_expired
                    .saturating_add(stats.num_dropped_on_age as u64);

                gui_panels.live_txn_waterfall.out.resolv_ancient = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_ancient
                    .saturating_add(stats.num_dropped_on_capacity as u64);

                gui_panels.live_txn_waterfall.out.pack_expired = gui_panels
                    .live_txn_waterfall
                    .out
                    .pack_expired
                    .saturating_add(stats.num_too_old as u64);

                gui_panels.live_txn_waterfall.out.pack_invalid = gui_panels
                    .live_txn_waterfall
                    .out
                    .pack_invalid
                    .saturating_add((stats.num_dropped_on_validate_locks
                        + stats.num_dropped_on_cleanup
                        + stats.num_dropped_on_already_processed
                        + stats.num_dropped_on_blacklisted_account
                        + stats.num_dropped_on_fee_payer
                        + stats.num_dropped_on_compute_budget) as u64);
            }
            GuiCoreMetrics::Scheduler(stats) => {
                gui_panels.live_txn_waterfall.out.resolv_lut_failed = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_lut_failed
                    .saturating_add(stats.num_dropped_on_sanitization as u64);

                gui_panels.live_txn_waterfall.out.resolv_expired = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_expired
                    .saturating_add(stats.num_dropped_on_age_and_status as u64);

                gui_panels.live_txn_waterfall.out.resolv_ancient = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_ancient
                    .saturating_add(stats.num_dropped_on_capacity as u64);

                gui_panels.live_txn_waterfall.out.pack_invalid = gui_panels
                    .live_txn_waterfall
                    .out
                    .pack_invalid
                    .saturating_add((stats.num_dropped_on_receive
                        + stats.num_dropped_on_validate_locks
                        + stats.num_dropped_on_receive_transaction_checks
                        + stats.num_dropped_on_clear
                    ) as u64);
            }

            GuiCoreMetrics::Scheduler2(stats) => {
                gui_panels.live_txn_waterfall.out.resolv_lut_failed = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_lut_failed
                    .saturating_add(stats.num_dropped_on_sanitization as u64);

                gui_panels.live_txn_waterfall.out.resolv_expired = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_expired
                    .saturating_add(stats.num_dropped_on_age_and_status as u64);

                gui_panels.live_txn_waterfall.out.resolv_ancient = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_ancient
                    .saturating_add(stats.num_dropped_on_capacity as u64);

                gui_panels.live_txn_waterfall.out.pack_invalid = gui_panels
                    .live_txn_waterfall
                    .out
                    .pack_invalid
                    .saturating_add((stats.num_dropped_on_receive
                        + stats.num_dropped_on_validate_locks
                        + stats.num_dropped_on_receive_transaction_checks
                        + stats.num_dropped_on_clear
                    ) as u64);
            }

            GuiCoreMetrics::StandardScheduler(stats) => {
                gui_panels.live_txn_waterfall.out.resolv_lut_failed = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_lut_failed
                    .saturating_add(stats.num_dropped_on_parsing_and_sanitization.0 as u64);

                gui_panels.live_txn_waterfall.out.resolv_expired = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_expired
                    .saturating_add(stats.num_dropped_on_receive_age.0 as u64);

                gui_panels.live_txn_waterfall.out.resolv_ancient = gui_panels
                    .live_txn_waterfall
                    .out
                    .resolv_ancient
                    .saturating_add(stats.num_dropped_on_capacity.0 as u64);

                let remaining_drops = stats.num_dropped_on_filter_key
                + stats.num_dropped_on_receive
                + stats.num_dropped_on_validate_locks
                + stats.num_dropped_on_receive_compute_budget
                + stats.num_dropped_on_receive_already_processed
                + stats.num_dropped_on_receive_fee_payer
                + stats.num_dropped_on_clear
                + stats.num_dropped_on_clean;

                gui_panels.live_txn_waterfall.out.pack_invalid = gui_panels
                    .live_txn_waterfall
                    .out
                    .pack_invalid
                    .saturating_add(remaining_drops.0 as u64);
            }

            GuiCoreMetrics::ConsumeWorker(stats) => {
                gui_panels.live_txn_waterfall.out.bank_invalid = gui_panels
                    .live_txn_waterfall
                    .out
                    .bank_invalid
                    .saturating_add((stats.transactions_attempted_processing_count.load(Ordering::Relaxed) - stats.processed_transactions_count.load(Ordering::Relaxed)) as u64);

                gui_panels.live_txn_waterfall.out.block_success = gui_panels
                    .live_txn_waterfall
                    .out
                    .block_success
                    .saturating_add(stats.processed_with_successful_result_count.load(Ordering::Relaxed) as u64);

                gui_panels.live_txn_waterfall.out.block_fail = gui_panels
                    .live_txn_waterfall
                    .out
                    .block_fail
                    .saturating_add(
                        (stats.processed_transactions_count.load(Ordering::Relaxed) - stats.processed_with_successful_result_count.load(Ordering::Relaxed)
                    ) as u64);
            }

            GuiCoreMetrics::BankingStageVote(stats) => {
                gui_panels.live_txn_waterfall.in_.gossip = gui_panels
                    .live_txn_waterfall
                    .in_
                    .gossip
                    .saturating_add(stats.gossip_receive_and_buffer_packets_count);

                gui_panels.live_txn_waterfall.out.dedup_duplicate = gui_panels
                    .live_txn_waterfall
                    .out
                    .dedup_duplicate
                    .saturating_add(stats.tpu_dropped_packets_count)
                    .saturating_add(stats.gossip_dropped_packets_count);

                gui_panels.live_txn_waterfall.out.pack_already_executed = gui_panels
                    .live_txn_waterfall
                    .out
                    .pack_already_executed
                    .saturating_add(stats.dropped_forward_gossip_packets_count)
                    .saturating_add(stats.dropped_forward_tpu_packets_count);
            }

            GuiCoreMetrics::VoteWorker(stats) => {
                gui_panels.live_txn_waterfall.out.verify_failed = gui_panels
                    .live_txn_waterfall
                    .out
                    .verify_failed
                    .saturating_add((stats.newly_failed_sigverify_count) as u64);

                gui_panels.live_txn_waterfall.out.verify_parse = gui_panels
                    .live_txn_waterfall
                    .out
                    .verify_parse
                    .saturating_add((stats.failed_sanitization_count + stats.failed_prioritization_count + stats.invalid_votes_count) as u64);

                gui_panels.live_txn_waterfall.out.pack_expired = gui_panels
                    .live_txn_waterfall
                    .out
                    .pack_expired
                    .saturating_add(stats.retryable_packets_filtered_count as u64);

                gui_panels.live_txn_waterfall.out.block_success = gui_panels
                    .live_txn_waterfall
                    .out
                    .block_success
                    .saturating_add(stats.committed_transactions_with_successful_result_count);

                gui_panels.live_txn_waterfall.out.bank_invalid = gui_panels
                    .live_txn_waterfall
                    .out
                    .bank_invalid
                    .saturating_add(stats.nonretryable_errored_transactions_count + stats.executed_transactions_failed_commit_count);

                gui_panels.live_txn_waterfall.out.block_fail = gui_panels
                    .live_txn_waterfall
                    .out
                    .block_fail
                    .saturating_add(stats.committed_transactions_count - stats.committed_transactions_with_successful_result_count);     
            }
            GuiCoreMetrics::FetchStageForwardDropped(count) => {
                gui_panels.live_txn_waterfall.out.quic_overrun = gui_panels
                    .live_txn_waterfall
                    .out
                    .quic_overrun
                    .saturating_add(count as u64);
            }
            GuiCoreMetrics::FetchStageForwardDiscard(count) => {
                gui_panels.live_txn_waterfall.out.quic_abandoned = gui_panels
                    .live_txn_waterfall
                    .out
                    .quic_abandoned
                    .saturating_add(count as u64);
            }
            GuiCoreMetrics::FetchStageManagerForwardDropped(count) => {
                gui_panels.live_txn_waterfall.out.quic_abandoned = gui_panels
                    .live_txn_waterfall
                    .out
                    .quic_abandoned
                    .saturating_add(count as u64);
            }
        }
    }

    fn update_leader_slot_state(
        shared_leader_state: &SharedLeaderState,
        last_leader_slot: &mut Option<Slot>,
        next_leader_slot: &mut Option<Slot>,
        retained_snapshot: &mut RetainedSnapshot,
        gui_panels: &mut GuiPanels,
        slot_txn_store: &mut SlotTxnStore,
    ) {
        let live_txn_waterfall = &mut gui_panels.live_txn_waterfall;
        let leader_state = shared_leader_state.load();
        *next_leader_slot = leader_state
            .next_leader_slot_range()
            .map(|(start, _)| start);

        let current_leader_slot = leader_state.working_bank().map(|bank| bank.slot());
        if *last_leader_slot != current_leader_slot {
            let now = wallclock_timestamp_nanos();
            if let Some(prev) = *last_leader_slot {
                slot_txn_store.set_slot_end(prev, now);
            }
            if let Some(slot) = current_leader_slot {
                let ns_per_slot = leader_state
                    .working_bank()
                    .map(|bank| bank.ns_per_slot)
                    .unwrap_or(0);
                slot_txn_store.set_slot_start(slot, now, ns_per_slot);
            }
            if last_leader_slot.is_some() {
                let retained = RetainedSnapshot::capture_from(live_txn_waterfall);
                *retained_snapshot = retained;
                *live_txn_waterfall = TxnWaterfall::default();
                Self::sync_pack_retained(gui_panels);
            }
            *last_leader_slot = current_leader_slot;
        }
    }
}
