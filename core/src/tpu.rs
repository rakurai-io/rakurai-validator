//! The `tpu` module implements the Transaction Processing Unit, a
//! multi-stage transaction processing pipeline in software.

// allow multiple connections for NAT and any open/close overlap
use crate::banking_stage::{DecisionState, RakuraiConfig, house_keeper::HouseKeeper};
#[deprecated(
    since = "2.2.0",
    note = "Use solana_streamer::quic::DEFAULT_MAX_QUIC_CONNECTIONS_PER_PEER instead"
)]
use {
    crate::{
        admin_rpc_post_init::{KeyUpdaterType, KeyUpdaters},
        bam_dependencies::{BamConnectionState, BamDependencies},
        bam_manager::BamManager,
        banking_stage::{
            BankingControlMsg, BankingStage, BankingStageHandle,
            consumer::TipProcessingDependencies, reward_distributor::RewardDistributionConfig,
            transaction_scheduler::scheduler_controller::SchedulerConfig,
        },
        banking_trace::{Channels, TracerThread},
        bundle_sigverify_stage::BundleSigverifyStage,
        bundle_stage::{BundleStage, bundle_account_locker::BundleAccountLocker},
        cluster_info_vote_listener::{
            ClusterInfoVoteListener, DuplicateConfirmedSlotsSender, GossipVerifiedVoteHashSender,
            VoteTracker,
        },
        fetch_stage::FetchStage,
        forwarding_stage::{
            ForwardAddressGetter, ForwardingClientConfig, SpawnForwardingStageResult,
            spawn_forwarding_stage,
        },
        gui::{GuiCoreMetrics, GuiTxnEvent},
        proxy::{
            block_engine_stage::{BlockBuilderFeeInfo, BlockEngineConfig, BlockEngineStage},
            fetch_stage_manager::FetchStageManager,
            relayer_stage::{RelayerConfig, RelayerStage},
        },
        sigverify_stage::SigVerifyStage,
        staked_nodes_updater_service::StakedNodesUpdaterService,
        tip_manager::{TipManager, TipManagerConfig},
        tpu_entry_notifier::TpuEntryNotifier,
        validator::{BlockProductionMethod, ClientMode, GeneratorConfig},
    },
    agave_banking_stage_ingress_types::SchedulerPriorityFloor,
    agave_votor::event::VotorEventSender,
    agave_votor_messages::VerifiedVoterSlotsSender,
    agave_xdp::transmitter::XdpSender,
    ahash::HashSet as AHashSet,
    arc_swap::ArcSwap,
    crossbeam_channel::{Receiver, Sender, bounded, unbounded},
    solana_clock::Slot,
    solana_gossip::cluster_info::ClusterInfo,
    solana_keypair::Keypair,
    solana_ledger::{
        blockstore::Blockstore, blockstore_processor::TransactionStatusSender,
        entry_notifier_service::EntryNotifierSender,
    },
    solana_poh::{
        poh_recorder::{PohRecorder, WorkingBankEntryOrMarker},
        transaction_recorder::TransactionRecorder,
    },
    solana_pubkey::Pubkey,
    solana_rpc::{
        optimistically_confirmed_bank_tracker::BankNotificationSenderConfig,
        rpc_subscriptions::RpcSubscriptions,
    },
    solana_runtime::{
        bank_forks::BankForks,
        prioritization_fee_cache::PrioritizationFeeCache,
        vote_sender_types::{ReplayVoteReceiver, ReplayVoteSender},
    },
    solana_signer::Signer,
    solana_streamer::{
        evicting_sender::EvictingSender,
        quic::{
            GuiStreamerMetrics, SimpleQosQuicStreamerConfig, SpawnServerResult,
            SwQosQuicStreamerConfig, spawn_simple_qos_server, spawn_stake_weighted_qos_server,
        },
        quic_socket::QuicSocket,
        streamer::StakedNodes,
    },
    solana_turbine::{
        ShredReceiverAddresses, XdpSender as TurbineXdpSender,
        broadcast_stage::{BroadcastStage, BroadcastStageType},
    },
    std::{
        collections::{HashMap, HashSet},
        net::{Ipv4Addr, SocketAddr, UdpSocket},
        num::NonZeroUsize,
        path::PathBuf,
        sync::{
            Arc, Mutex, RwLock,
            atomic::{AtomicBool, AtomicU8},
        },
        thread::{self, JoinHandle},
    },
    tokio::sync::mpsc,
    tokio_util::sync::CancellationToken,
};

pub struct TpuSockets {
    pub vote: Vec<UdpSocket>,
    pub broadcast: Vec<UdpSocket>,
    pub transactions_quic: Vec<UdpSocket>,
    pub transactions_forwards_quic: Vec<UdpSocket>,
    pub vote_quic: Vec<UdpSocket>,
    /// Client-side socket for the forwarding votes.
    pub vote_forwarding_client: UdpSocket,
}

// Conservatively allow 20 TPS per validator.
pub const MAX_VOTES_PER_SECOND: u64 = 20;

/// Size of the channel between streamer and TPU sigverify stage. The values have been selected to
/// be conservative max of obsersed on mnb during high-load events.
const TPU_CHANNEL_SIZE: usize = 50_000;

/// Size of the channel between the vote streamer and the TPU sigverify stage.
/// Chosen based on nominal voting load for a cluster with ~2000 validators + some margin.
pub(crate) const TPU_VOTE_CHANNEL_SIZE: usize = 4_000;

/// Size of the channel between the TPU forwards streamer and the fetch stage.
/// Mirrors `TPU_CHANNEL_SIZE`; the streamer uses `try_send`, so an over-full
/// channel drops packets (tracked via streamer metrics) rather than blocking.
const TPU_FORWARD_CHANNEL_SIZE: usize = 50_000;

pub struct Tpu {
    fetch_stage: FetchStage,
    cluster_info_vote_listener: ClusterInfoVoteListener,
    sigverify_stage: SigVerifyStage,
    banking_stage: BankingStageHandle,
    house_keeper_thread: HouseKeeper,
    forwarding_stage: JoinHandle<()>,
    broadcast_stage: BroadcastStage,
    tpu_quic_t: thread::JoinHandle<()>,
    tpu_forwards_quic_t: thread::JoinHandle<()>,
    tpu_entry_notifier: Option<TpuEntryNotifier>,
    staked_nodes_updater_service: StakedNodesUpdaterService,
    tracer_thread_hdl: TracerThread,
    tpu_vote_quic_t: thread::JoinHandle<()>,
    relayer_stage: RelayerStage,
    block_engine_stage: BlockEngineStage,
    fetch_stage_manager: FetchStageManager,
    bundle_stage: BundleStage,
    bundle_sigverify_stage: BundleSigverifyStage,
    bam_manager: BamManager,
}

impl Tpu {
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_client(
        cluster_info: &Arc<ClusterInfo>,
        poh_recorder: &Arc<RwLock<PohRecorder>>,
        transaction_recorder: TransactionRecorder,
        entry_receiver: Receiver<WorkingBankEntryOrMarker>,
        retransmit_slots_receiver: Receiver<Slot>,
        sockets: TpuSockets,
        subscriptions: Option<Arc<RpcSubscriptions>>,
        transaction_status_sender: Option<TransactionStatusSender>,
        entry_notification_sender: Option<EntryNotifierSender>,
        blockstore: Arc<Blockstore>,
        broadcast_type: &BroadcastStageType,
        leader_schedule_cache: Arc<solana_ledger::leader_schedule_cache::LeaderScheduleCache>,
        turbine_xdp_sender: Option<TurbineXdpSender>,
        quic_xdp_sender: Option<(XdpSender, Ipv4Addr)>,
        exit: Arc<AtomicBool>,
        shred_version: u16,
        vote_tracker: Arc<VoteTracker>,
        bank_forks: Arc<RwLock<BankForks>>,
        verified_voter_slots_sender: VerifiedVoterSlotsSender,
        gossip_verified_vote_hash_sender: GossipVerifiedVoteHashSender,
        replay_vote_receiver: ReplayVoteReceiver,
        replay_vote_sender: ReplayVoteSender,
        bank_notification_sender: Option<BankNotificationSenderConfig>,
        duplicate_confirmed_slot_sender: DuplicateConfirmedSlotsSender,
        tpu_forwarding_client_config: ForwardingClientConfig,
        keypair: &Keypair,
        log_messages_bytes_limit: Option<usize>,
        staked_nodes: &Arc<RwLock<StakedNodes>>,
        shared_staked_nodes_overrides: Arc<RwLock<HashMap<Pubkey, u64>>>,
        banking_tracer_channels: Channels,
        tracer_thread_hdl: TracerThread,
        tpu_quic_server_config: SwQosQuicStreamerConfig,
        tpu_fwd_quic_server_config: SwQosQuicStreamerConfig,
        vote_quic_server_config: SimpleQosQuicStreamerConfig,
        prioritization_fee_cache: Option<Arc<PrioritizationFeeCache>>,
        tpu_sigverify_threads: NonZeroUsize,
        block_production_method: BlockProductionMethod,
        block_production_num_workers: NonZeroUsize,
        block_production_scheduler_config: SchedulerConfig,
        filter_keys: Arc<HashSet<Pubkey>>,
        enable_block_production_forwarding: bool,
        _generator_config: Option<GeneratorConfig>, /* vestigial code for replay invalidator */
        key_notifiers: Arc<RwLock<KeyUpdaters>>,
        banking_control_receiver: mpsc::Receiver<BankingControlMsg>,
        scheduler_bindings: Option<(PathBuf, mpsc::Sender<BankingControlMsg>)>,
        cancel: CancellationToken,
        votor_event_sender: VotorEventSender,
        block_engine_config: Arc<ArcSwap<BlockEngineConfig>>,
        secondary_block_engine_entries: Arc<
            ArcSwap<Vec<crate::proxy::block_engine_stage::BlockEngineEntry>>,
        >,
        block_engine_uuid_blocklist: Arc<ArcSwap<Vec<String>>>,
        relayer_config: Arc<ArcSwap<RelayerConfig>>,
        tip_manager_config: TipManagerConfig,
        shredstream_receiver_address: Arc<ArcSwap<Option<SocketAddr>>>,
        shred_receiver_addresses: Arc<ArcSwap<ShredReceiverAddresses>>,
        bam_shred_receiver_addresses: Arc<ArcSwap<ShredReceiverAddresses>>,
        multicast_receiver_address: Arc<ArcSwap<Option<SocketAddr>>>,
        bam_url: Arc<ArcSwap<Option<String>>>,
        reward_distribution_config: RewardDistributionConfig,
        rakurai_config: Arc<RwLock<RakuraiConfig>>,
        tx_io_check: Option<String>,
        oms_connector: bool,
        client_mode: Arc<Mutex<ClientMode>>,
        reset_rakurai: Arc<AtomicBool>,
        bundle_lifecycle_dump_enabled: Arc<AtomicBool>,
        vote_account: Pubkey,
        scheduling_strategy: Option<crate::banking_stage::SchedlingStrategy>,
        postpack_confirmation_config: Arc<RwLock<crate::banking_stage::PostPackConfirmationConfig>>,
        postpack_confirmation_active_entries: crate::banking_stage::PostPackConfirmationActiveEntries,
        post_pack_confirmation_uuid_blocklist: crate::banking_stage::PostPackConfirmationUuidBlocklist,
        gui_core_metrics_sender: Option<Sender<GuiCoreMetrics>>,
        gui_streamer_metrics_sender: Option<Sender<GuiStreamerMetrics>>,
        gui_txn_event_sender: Option<Sender<GuiTxnEvent>>,
    ) -> Self {
        let TpuSockets {
            vote: tpu_vote_sockets,
            broadcast: broadcast_sockets,
            transactions_quic: transactions_quic_sockets,
            transactions_forwards_quic: transactions_forwards_quic_sockets,
            vote_quic: tpu_vote_quic_sockets,
            vote_forwarding_client: vote_forwarding_client_socket,
        } = sockets;

        // [----------]
        // [-- QUIC --] \
        // [----------]  \____     [-----------------------]     [--------------------]     [------------------]
        //                    ---- [-- FetchStageManager --] --> [-- SigverifyStage --] --> [-- BankingStage --]
        // [--------------]  /     [-----------------------]     [--------------------]     [------------------]
        // [-- Vortexor --] /
        // [--------------]
        //
        //             fetch_stage_manager_*                packet_receiver

        // Packets from fetch stage and quic server are intercepted and sent through fetch_stage_manager
        // If relayer is connected, packets are dropped. If not, packets are forwarded on to packet_sender
        let (fetch_stage_manager_sender, fetch_stage_manager_receiver) = bounded(TPU_CHANNEL_SIZE);
        let (sigverify_stage_sender, sigverify_stage_receiver) = bounded(TPU_CHANNEL_SIZE);

        let (vote_packet_sender, vote_packet_receiver) = bounded(TPU_VOTE_CHANNEL_SIZE);
        let evicting_vote_sender =
            EvictingSender::new(vote_packet_sender.clone(), vote_packet_receiver.clone());
        let (forwarded_packet_sender, forwarded_packet_receiver) =
            bounded(TPU_FORWARD_CHANNEL_SIZE);
        let fetch_stage = FetchStage::new_with_sender(
            tpu_vote_sockets,
            exit.clone(),
            &fetch_stage_manager_sender,
            &evicting_vote_sender,
            forwarded_packet_receiver,
            poh_recorder,
            None, // coalesce
            gui_core_metrics_sender.clone(),
            gui_streamer_metrics_sender.clone(),
        );

        let staked_nodes_updater_service = StakedNodesUpdaterService::new(
            exit.clone(),
            bank_forks.clone(),
            staked_nodes.clone(),
            shared_staked_nodes_overrides,
        );

        let Channels {
            non_vote_sender: banking_stage_sender,
            non_vote_receiver: banking_stage_receiver,
            tpu_vote_sender,
            tpu_vote_receiver,
            gossip_vote_sender,
            gossip_vote_receiver,
        } = banking_tracer_channels;

        // Streamer for Votes:
        let quic_vote_sockets: Vec<QuicSocket> =
            tpu_vote_quic_sockets.into_iter().map(Into::into).collect();
        let (
            SpawnServerResult {
                endpoints: _,
                thread: tpu_vote_quic_t,
                key_updater: vote_streamer_key_updater,
            },
            _banlist,
        ) = spawn_simple_qos_server(
            "solQuicTVo",
            "quic_streamer_tpu_vote",
            quic_vote_sockets,
            keypair,
            vote_packet_sender,
            staked_nodes.clone(),
            vote_quic_server_config.quic_streamer_config,
            vote_quic_server_config.qos_config,
            cancel.clone(),
            gui_streamer_metrics_sender.clone(),
        )
        .unwrap();

        // We check on validator startup that XDP is not mixed with multihoming, so by construction
        // at this moment all the transactions_quic_sockets and transactions_forwards_quic_sockets
        // have the same bind IP:PORT.

        // Streamer for TPU
        let transactions_quic_sockets =
            into_quic_sockets(transactions_quic_sockets, quic_xdp_sender.clone());
        let SpawnServerResult {
            endpoints: _,
            thread: tpu_quic_t,
            key_updater,
        } = spawn_stake_weighted_qos_server(
            "solQuicTpu",
            "quic_streamer_tpu",
            transactions_quic_sockets,
            keypair,
            fetch_stage_manager_sender,
            staked_nodes.clone(),
            tpu_quic_server_config.quic_streamer_config,
            tpu_quic_server_config.qos_config,
            cancel.clone(),
            gui_streamer_metrics_sender.clone(),
        )
        .unwrap();

        // Streamer for TPU forward
        let transactions_forwards_quic_sockets =
            into_quic_sockets(transactions_forwards_quic_sockets, quic_xdp_sender);
        let SpawnServerResult {
            endpoints: _,
            thread: tpu_forwards_quic_t,
            key_updater: forwards_key_updater,
        } = spawn_stake_weighted_qos_server(
            "solQuicTpuFwd",
            "quic_streamer_tpu_forwards",
            transactions_forwards_quic_sockets,
            keypair,
            forwarded_packet_sender,
            staked_nodes.clone(),
            tpu_fwd_quic_server_config.quic_streamer_config,
            tpu_fwd_quic_server_config.qos_config,
            cancel,
            gui_streamer_metrics_sender,
        )
        .unwrap();

        let (forward_stage_sender, forward_stage_receiver) = bounded(50_000);

        let scheduler_priority_floor = Arc::new(SchedulerPriorityFloor::new());
        const TX_IO_CHANNEL_SZIE: usize = 100_000;
        let enable_tx_io_check = tx_io_check.is_some();
        let (input_tx_signature_sender, input_tx_signature_receiver) = if enable_tx_io_check {
            let (input_tx_signature_sender, input_tx_signature_receiver) =
                bounded(TX_IO_CHANNEL_SZIE);
            (
                Some((input_tx_signature_sender, exit.clone())),
                Some(input_tx_signature_receiver),
            )
        } else {
            (None, None)
        };

        let (sigverify_stage, gossip_sigverify_handle) = SigVerifyStage::new(
            sigverify_stage_receiver,
            vote_packet_receiver,
            banking_stage_sender.clone(),
            tpu_vote_sender,
            forward_stage_sender.clone(),
            tpu_sigverify_threads,
            enable_block_production_forwarding,
            bank_forks.read().unwrap().sharable_banks(),
            Some(scheduler_priority_floor.clone()),
            input_tx_signature_sender.clone(),
            gui_core_metrics_sender.clone(),
        );

        let (output_tx_signature_sender, output_tx_signature_receiver) =
            if enable_tx_io_check || oms_connector {
                let (output_tx_signature_sender, output_tx_signature_receiver) =
                    bounded(TX_IO_CHANNEL_SZIE);
                (
                    Some(output_tx_signature_sender),
                    Some(output_tx_signature_receiver),
                )
            } else {
                (None, None)
            };

        let sigverify_threadpool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(tpu_sigverify_threads.get())
                .thread_name(|i| format!("solBndlSigV{i:02}"))
                .build()
                .expect("new rayon threadpool"),
        );

        let block_builder_fee_info = Arc::new(ArcSwap::from_pointee(BlockBuilderFeeInfo {
            block_builder: cluster_info.keypair().pubkey(),
            block_builder_commission: 0,
        }));

        let (unverified_bundle_sender, unverified_bundle_receiver) = bounded(16_384);
        let bam_enabled = Arc::new(AtomicU8::new(BamConnectionState::Disconnected as u8));

        let block_engine_stage = BlockEngineStage::new(
            block_engine_config.clone(),
            secondary_block_engine_entries,
            block_engine_uuid_blocklist,
            bank_forks.read().unwrap().sharable_banks(),
            unverified_bundle_sender,
            cluster_info.clone(),
            sigverify_stage_sender.clone(),
            banking_stage_sender.clone(),
            exit.clone(),
            &block_builder_fee_info,
            shredstream_receiver_address.clone(),
            bam_enabled.clone(),
            input_tx_signature_sender.clone(),
            gui_core_metrics_sender.clone(),
            bundle_lifecycle_dump_enabled.clone(),
            vote_account,
        );
        let (verified_bundle_sender, verified_bundle_receiver) = bounded(16_384);
        let bundle_sigverify_stage = BundleSigverifyStage::new(
            sigverify_threadpool.clone(),
            unverified_bundle_receiver,
            verified_bundle_sender,
            exit.clone(),
            banking_stage_sender.clone(),
            gui_core_metrics_sender.clone(),
        );

        let bam_tpu_info = Arc::new(ArcSwap::new(Arc::new(None)));
        let (heartbeat_tx, heartbeat_rx) = bounded(TPU_CHANNEL_SIZE);
        let fetch_stage_manager = FetchStageManager::new(
            cluster_info.clone(),
            heartbeat_rx,
            fetch_stage_manager_receiver,
            sigverify_stage_sender.clone(),
            exit.clone(),
            bam_enabled.clone(),
            cluster_info.my_contact_info().clone(),
            bam_tpu_info.clone(),
            gui_core_metrics_sender.clone(),
        );

        let relayer_stage = RelayerStage::new(
            relayer_config,
            cluster_info.clone(),
            heartbeat_tx,
            sigverify_stage_sender,
            exit.clone(),
        );
        let cluster_info_vote_listener = ClusterInfoVoteListener::new(
            exit.clone(),
            cluster_info.clone(),
            gossip_sigverify_handle,
            gossip_vote_sender,
            vote_tracker,
            bank_forks.clone(),
            subscriptions,
            verified_voter_slots_sender,
            gossip_verified_vote_hash_sender,
            replay_vote_receiver,
            blockstore.clone(),
            bank_notification_sender,
            duplicate_confirmed_slot_sender,
        );

        let bundle_account_locker = BundleAccountLocker::default();

        let tip_manager = TipManager::new(tip_manager_config);
        let filter_keys = {
            let mut filter_keys = filter_keys.as_ref().clone();
            filter_keys.insert(tip_manager.tip_payment_program_id());
            filter_keys.insert(reward_distribution_config.rakurai_tip_manager_program_id);
            Arc::new(filter_keys)
        };
        let (bam_batch_sender, bam_batch_receiver) = bounded(100_000);
        let (bam_outbound_sender, bam_outbound_receiver) = mpsc::channel(100_000);
        let bam_dependencies = BamDependencies {
            bam_enabled: bam_enabled.clone(),
            batch_sender: bam_batch_sender,
            batch_receiver: bam_batch_receiver,
            outbound_sender: bam_outbound_sender,
            cluster_info: cluster_info.clone(),
            block_builder_fee_info: Arc::new(ArcSwap::from_pointee(BlockBuilderFeeInfo::default())),
            bam_node_pubkey: Arc::new(ArcSwap::from_pointee(Pubkey::default())),
            bank_forks: bank_forks.clone(),
            bam_tpu_info,
            bam_shred_receiver_addresses: bam_shred_receiver_addresses.clone(),
        };

        let shared_decision = (
            Arc::new(RwLock::new(DecisionState::Hold)),
            Arc::new(AtomicBool::new(false)),
        );
        let nonce_packets = Arc::new(RwLock::new(HashMap::new()));
        let (nonce_packet_sender, nonce_packet_receiver) = unbounded();
        let scheduler_postpack_conf_signatures = Arc::new(RwLock::new(HashMap::new()));

        let banking_stage = BankingStage::new_num_threads(
            block_production_method,
            poh_recorder.clone(),
            transaction_recorder.clone(),
            banking_stage_receiver,
            tpu_vote_receiver,
            gossip_vote_receiver,
            banking_control_receiver,
            block_production_num_workers,
            block_production_scheduler_config,
            transaction_status_sender.clone(),
            replay_vote_sender.clone(),
            log_messages_bytes_limit,
            bank_forks.clone(),
            prioritization_fee_cache.clone(),
            filter_keys.clone(),
            scheduler_priority_floor,
            bundle_account_locker.clone(),
            Some(TipProcessingDependencies {
                tip_manager: tip_manager.clone(),
                last_tip_updated_slot: Arc::new(Mutex::new(0)),
                block_builder_fee_info: bam_dependencies.block_builder_fee_info.clone(),
                cluster_info: cluster_info.clone(),
                bundle_account_locker: bundle_account_locker.clone(),
            }),
            Some(bam_dependencies.clone()),
            cluster_info,
            blockstore.clone(),
            reward_distribution_config,
            rakurai_config,
            input_tx_signature_sender.clone(),
            output_tx_signature_sender,
            shared_decision.clone(),
            exit.clone(),
            client_mode.clone(),
            reset_rakurai.clone(),
            scheduling_strategy,
            nonce_packets.clone(),
            nonce_packet_receiver,
            postpack_confirmation_config,
            postpack_confirmation_active_entries,
            post_pack_confirmation_uuid_blocklist,
            scheduler_postpack_conf_signatures.clone(),
            gui_core_metrics_sender.clone(),
            gui_txn_event_sender.clone(),
        );

        // House keeper
        let house_keeper_thread = HouseKeeper::new(
            input_tx_signature_receiver,
            output_tx_signature_receiver,
            tx_io_check,
            oms_connector,
            shared_decision,
            exit.clone(),
        );

        #[cfg(unix)]
        if let Some((path, banking_control_sender)) = scheduler_bindings {
            super::scheduler_bindings_server::spawn(&path, banking_control_sender);
        }
        #[cfg(not(unix))]
        assert!(scheduler_bindings.is_none());

        let SpawnForwardingStageResult {
            join_handle: forwarding_stage,
            client_updater,
        } = spawn_forwarding_stage(
            forward_stage_receiver,
            tpu_forwarding_client_config,
            vote_forwarding_client_socket,
            bank_forks.read().unwrap().sharable_banks(),
            ForwardAddressGetter::new(cluster_info.clone(), poh_recorder.clone()),
        );

        let bundle_stage = BundleStage::new(
            cluster_info,
            bank_forks.clone(),
            poh_recorder,
            transaction_recorder,
            verified_bundle_receiver,
            transaction_status_sender,
            replay_vote_sender,
            log_messages_bytes_limit,
            exit.clone(),
            tip_manager,
            bundle_account_locker,
            &block_builder_fee_info,
            prioritization_fee_cache.clone(),
            filter_keys.iter().copied().collect::<AHashSet<_>>(),
            nonce_packets,
            nonce_packet_sender,
            scheduler_postpack_conf_signatures,
            block_engine_config,
            bundle_lifecycle_dump_enabled,
            gui_core_metrics_sender,
            gui_txn_event_sender,
        );

        let bam_manager = BamManager::new(
            exit.clone(),
            bam_url,
            bam_dependencies,
            bam_outbound_receiver,
            poh_recorder.clone(),
            key_notifiers.clone(),
            banking_stage_sender.clone(),
            client_mode.clone(),
        );

        let (entry_receiver, tpu_entry_notifier) =
            if let Some(entry_notification_sender) = entry_notification_sender {
                let (broadcast_entry_sender, broadcast_entry_receiver) = bounded(TPU_CHANNEL_SIZE);
                let tpu_entry_notifier = TpuEntryNotifier::new(
                    entry_receiver,
                    entry_notification_sender,
                    broadcast_entry_sender,
                    exit.clone(),
                );
                (broadcast_entry_receiver, Some(tpu_entry_notifier))
            } else {
                (entry_receiver, None)
            };

        let broadcast_stage = broadcast_type.new_broadcast_stage(
            broadcast_sockets,
            cluster_info.clone(),
            entry_receiver,
            retransmit_slots_receiver,
            exit,
            blockstore,
            bank_forks,
            leader_schedule_cache,
            shred_version,
            turbine_xdp_sender,
            votor_event_sender,
            shredstream_receiver_address,
            shred_receiver_addresses,
            bam_shred_receiver_addresses,
            multicast_receiver_address,
        );

        let mut key_notifiers = key_notifiers.write().unwrap();
        key_notifiers.add(KeyUpdaterType::Tpu, key_updater);
        key_notifiers.add(KeyUpdaterType::TpuForwards, forwards_key_updater);
        key_notifiers.add(KeyUpdaterType::TpuVote, vote_streamer_key_updater);
        key_notifiers.add(KeyUpdaterType::Forward, client_updater);

        Self {
            fetch_stage,
            cluster_info_vote_listener,
            sigverify_stage,
            banking_stage,
            house_keeper_thread,
            forwarding_stage,
            broadcast_stage,
            tpu_quic_t,
            tpu_forwards_quic_t,
            tpu_entry_notifier,
            staked_nodes_updater_service,
            tracer_thread_hdl,
            tpu_vote_quic_t,
            block_engine_stage,
            relayer_stage,
            fetch_stage_manager,
            bundle_stage,
            bundle_sigverify_stage,
            bam_manager,
        }
    }

    pub fn join(self) -> thread::Result<()> {
        let results = vec![
            self.fetch_stage.join(),
            self.cluster_info_vote_listener.join(),
            self.sigverify_stage.join(),
            self.banking_stage.join(),
            self.house_keeper_thread.join(),
            self.forwarding_stage.join(),
            self.staked_nodes_updater_service.join(),
            self.tpu_quic_t.join(),
            self.tpu_forwards_quic_t.join(),
            self.tpu_vote_quic_t.join(),
            self.bundle_stage.join(),
            self.bundle_sigverify_stage.join(),
            self.relayer_stage.join(),
            self.block_engine_stage.join(),
            self.fetch_stage_manager.join(),
            self.bam_manager.join(),
        ];
        let broadcast_result = self.broadcast_stage.join();
        for result in results {
            result?;
        }
        if let Some(tpu_entry_notifier) = self.tpu_entry_notifier {
            tpu_entry_notifier.join()?;
        }
        let _ = broadcast_result?;
        if let Some(tracer_thread_hdl) = self.tracer_thread_hdl
            && let Err(tracer_result) = tracer_thread_hdl.join()?
        {
            error!(
                "banking tracer thread returned error after successful thread join: \
                 {tracer_result:?}"
            );
        }
        Ok(())
    }
}

fn into_quic_sockets(
    sockets: impl IntoIterator<Item = UdpSocket>,
    quic_xdp_sender: Option<(XdpSender, Ipv4Addr)>,
) -> impl Iterator<Item = QuicSocket> {
    sockets
        .into_iter()
        .map(move |socket| match &quic_xdp_sender {
            Some((xdp_sender, fallback_src_ip)) => {
                QuicSocket::with_xdp(socket, *fallback_src_ip, xdp_sender.clone())
            }
            None => QuicSocket::from(socket),
        })
}
