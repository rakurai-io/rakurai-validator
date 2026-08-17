//! Maintains a connection to the Block Engine.
//!
//! The Block Engine is responsible for the following:
//! - Acts as a system that sends high profit bundles and transactions to a validator.
//! - Sends transactions and bundles to the validator.

use {
    crate::{
        bam_dependencies::BamConnectionState,
        banking_trace::BankingPacketSender,
        bundle_stage::{BundleDropReason, BundleExecutionStats},
        gui::GuiCoreMetrics,
        packet_bundle::PacketBundle,
        proto_packet_to_packet,
        proxy::{
            ProxyError,
            auth::{AuthInterceptor, auth_client_from_endpoint, maybe_refresh_auth_tokens},
            endpoint_from_url, sanitize_status_message_for_influx,
        },
    },
    ahash::HashMapExt,
    arc_swap::ArcSwap,
    crossbeam_channel::Sender,
    governor::{DefaultDirectRateLimiter, Quota, RateLimiter},
    itertools::{Either, Itertools},
    jito_protos::proto::{
        auth::{Token, auth_service_client::AuthServiceClient},
        block_engine::{
            self, BlockBuilderFeeInfoRequest, BlockEngineEndpoint, GetBlockEngineEndpointRequest,
            block_engine_validator_client::BlockEngineValidatorClient,
        },
    },
    serde::{Deserialize, Serialize},
    solana_gossip::cluster_info::ClusterInfo,
    solana_keypair::Keypair,
    solana_perf::packet::{BytesPacket, PacketBatch},
    solana_pubkey::Pubkey,
    solana_runtime::{bank::Bank, bank_forks::BankForks},
    solana_signer::Signer,
    std::{
        collections::{HashMap, HashSet, hash_map::Entry},
        net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs},
        num::NonZeroU32,
        ops::AddAssign,
        str::FromStr,
        sync::{
            Arc, Mutex, RwLock,
            atomic::{AtomicBool, AtomicU8, Ordering},
        },
        thread::{self, Builder, JoinHandle},
        time::{Duration, Instant},
    },
    thiserror::Error,
    tokio::{
        task::{self, JoinSet},
        time::{interval, sleep, timeout},
    },
    tonic::{
        Streaming,
        codegen::InterceptedService,
        transport::{Channel, Endpoint},
    },
};

const CONNECTION_TIMEOUT_S: u64 = 10;
const CONNECTION_BACKOFF_S: u64 = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEngineEntry {
    pub url: String,
    pub uuid: String,
    #[serde(default)]
    pub bundle_rate_limit: BlockEngineBundleRateLimit,
}

pub fn parse_block_engine_entry(value: &str) -> Result<BlockEngineEntry, String> {
    let (url, uuid) = value
        .split_once(',')
        .ok_or_else(|| format!("expected url,uuid format, got: {value}"))?;
    if url.is_empty() || uuid.is_empty() {
        return Err(format!("url and uuid must be non-empty, got: {value}"));
    }
    Ok(BlockEngineEntry {
        url: url.to_string(),
        uuid: uuid.to_string(),
        ..Default::default()
    })
}

#[cfg(feature = "build_validator")]
mod ffi {
    use {super::BlockEngineEntry, solana_pubkey::Pubkey, solana_runtime::bank::Bank};

    unsafe extern "C" {
        #[allow(improper_ctypes)]
        #[allow(improper_ctypes_definitions)]
        pub fn load_secondary_block_engine_entries_from_bank(
            bank: &Bank,
            vote_account: &Pubkey,
        ) -> Option<Vec<BlockEngineEntry>>;
    }
}

/// Secondary block-engine URLs from the on-chain client-config PDA.
/// Implemented by rakurai_scheduler (same parser as tips / post-pack).
pub fn load_secondary_block_engine_entries_from_bank(
    bank: &Bank,
    vote_account: &Pubkey,
) -> Option<Vec<BlockEngineEntry>> {
    #[cfg(feature = "build_validator")]
    {
        // SAFETY: exported by rakurai_scheduler entrypoint from the same revision.
        return unsafe { ffi::load_secondary_block_engine_entries_from_bank(bank, vote_account) };
    }
    #[cfg(not(feature = "build_validator"))]
    {
        let _ = (bank, vote_account);
        None
    }
}

fn secondary_task_key(entry: &BlockEngineEntry) -> String {
    format!("{}|{}", entry.uuid, entry.url)
}

pub fn merged_secondary_block_engine_entries(
    admin_entries: &[BlockEngineEntry],
    onchain_entries: Option<Vec<BlockEngineEntry>>,
    blocklisted_uuids: &[String],
) -> Vec<BlockEngineEntry> {
    let blocklist: HashSet<&str> = blocklisted_uuids.iter().map(String::as_str).collect();
    let mut merged = admin_entries
        .iter()
        .filter(|entry| !blocklist.contains(entry.uuid.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let admin_uuids: HashSet<String> = merged.iter().map(|entry| entry.uuid.clone()).collect();
    if let Some(onchain_entries) = onchain_entries {
        for entry in onchain_entries {
            if !blocklist.contains(entry.uuid.as_str()) && !admin_uuids.contains(&entry.uuid) {
                merged.push(entry);
            }
        }
    }
    merged
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEngineUrlStatus {
    pub primary_url: String,
    pub admin_secondary_entries: Vec<BlockEngineEntry>,
    pub onchain_secondary_entries: Vec<BlockEngineEntry>,
    pub blocklisted_uuids: Vec<String>,
    pub active_secondary_entries: Vec<BlockEngineEntry>,
}

pub fn collect_block_engine_url_status(
    block_engine_config: &BlockEngineConfig,
    admin_secondary_entries: &[BlockEngineEntry],
    onchain_secondary_entries: Option<Vec<BlockEngineEntry>>,
    blocklisted_uuids: &[String],
) -> BlockEngineUrlStatus {
    let onchain_secondary_entries = onchain_secondary_entries.unwrap_or_default();
    BlockEngineUrlStatus {
        primary_url: block_engine_config.block_engine_url.clone(),
        admin_secondary_entries: admin_secondary_entries.to_vec(),
        onchain_secondary_entries: onchain_secondary_entries.clone(),
        blocklisted_uuids: blocklisted_uuids.to_vec(),
        active_secondary_entries: merged_secondary_block_engine_entries(
            admin_secondary_entries,
            Some(onchain_secondary_entries),
            blocklisted_uuids,
        ),
    }
}

#[derive(Default, Clone)]
pub struct BlockEngineStageStats {
    pub(crate) num_bundles: u64,
    num_bundle_packets: u64,
    num_packets: u64,
    num_empty_packets: u64,
    num_bundles_throttled: u64,
}

impl BlockEngineStageStats {
    pub(crate) fn report_with_url(&self, url: &str, is_primary: bool) {
        datapoint_info!(
            "block_engine_stage-stats",
            ("url", url, String),
            ("is_primary", is_primary, bool),
            ("num_bundles", self.num_bundles, i64),
            ("num_bundle_packets", self.num_bundle_packets, i64),
            ("num_packets", self.num_packets, i64),
            ("num_empty_packets", self.num_empty_packets, i64),
            ("num_bundles_throttled", self.num_bundles_throttled, i64)
        );
    }
}

#[derive(Clone, Default)]
pub struct BlockBuilderFeeInfo {
    pub block_builder: Pubkey,
    pub block_builder_commission: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEngineBundleRateLimit {
    /// Bundles admitted per `period_ms`. 0 = unlimited.
    pub max_bundles: u32,
    /// Quota window in milliseconds. 0 = unlimited.
    pub period_ms: u32,
    /// Token-bucket capacity. If 0 and quota is set, treat as equal to `max_bundles`.
    pub max_bundle_burst: u32,
}

fn maybe_bundle_limiter(
    limit: &BlockEngineBundleRateLimit,
) -> Option<Arc<DefaultDirectRateLimiter>> {
    if limit.max_bundles == 0 || limit.period_ms == 0 {
        return None;
    }
    let burst = if limit.max_bundle_burst == 0 {
        limit.max_bundles
    } else {
        limit.max_bundle_burst
    };
    let burst = NonZeroU32::new(burst)?;
    let cell_period =
        Duration::from_millis(limit.period_ms as u64).checked_div(limit.max_bundles)?;
    let quota = Quota::with_period(cell_period)?.allow_burst(burst);
    Some(Arc::new(RateLimiter::direct(quota)))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockEngineConfig {
    /// Block Engine URL
    pub block_engine_url: String,

    /// Stable identity for this connection (secondary entries); empty string for primary.
    pub block_engine_uuid: String,

    /// Disables Block Engine auto-configuration. This stops the validator client from using the most performant Block Engine region. Values provided to `--block-engine-url` will be used as-is.
    pub disable_block_engine_autoconfig: bool,

    /// If set then it will be assumed the backend verified packets so signature verification will be bypassed in the validator.
    pub trust_packets: bool,

    /// Ingress bundle rate limit for this connection. 0/0 = unlimited.
    /// Primary is never throttled. Admin/CLI secondaries default to unlimited;
    /// on-chain secondaries use the per-URL values from `rakurai_client_config`.
    pub bundle_rate_limit: BlockEngineBundleRateLimit,
}

pub struct BlockEngineStage {
    t_hdls: Vec<JoinHandle<()>>,
}
#[derive(Error, Debug)]
enum ProbeError {
    #[error(transparent)]
    BuildEndpoint(#[from] ProxyError),

    #[error("gRPC connect timeout")]
    ConnectTimeout,

    #[error("gRPC connect error: {0}")]
    Connect(#[from] tonic::transport::Error),

    #[error("gRPC request error: {0}")]
    Request(#[from] tonic::Status),

    #[error("no successful probe samples")]
    NoSuccessfulSamples,
}

impl BlockEngineStage {
    const CONNECTION_TIMEOUT: Duration = Duration::from_secs(CONNECTION_TIMEOUT_S);
    const CONNECTION_BACKOFF: Duration = Duration::from_secs(CONNECTION_BACKOFF_S);
    pub fn new(
        block_engine_config: Arc<ArcSwap<BlockEngineConfig>>,
        secondary_entries: Arc<ArcSwap<Vec<BlockEngineEntry>>>,
        blocklisted_uuids: Arc<ArcSwap<Vec<String>>>,
        bank_forks: Arc<RwLock<BankForks>>,
        // Channel that bundles get piped through.
        bundle_tx: Sender<Vec<PacketBundle>>,
        // The keypair stored here is used to sign auth challenges.
        cluster_info: Arc<ClusterInfo>,
        // Channel that non-trusted packets get piped through.
        packet_tx: Sender<PacketBatch>,
        // Channel that trusted packets get piped through.
        banking_packet_sender: BankingPacketSender,
        exit: Arc<AtomicBool>,
        block_builder_fee_info: &Arc<ArcSwap<BlockBuilderFeeInfo>>,
        shredstream_receiver_address: Arc<ArcSwap<Option<SocketAddr>>>,
        bam_enabled: Arc<AtomicU8>,
        input_tx_signature_sender: Option<(Sender<String>, Arc<AtomicBool>)>,
        gui_metrics_sender: Option<Sender<GuiCoreMetrics>>,
        bundle_lifecycle_dump_enabled: Arc<AtomicBool>,
        vote_account: Pubkey,
    ) -> Self {
        let secondary_task_exits = Arc::new(Mutex::new(HashMap::new()));

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let mut set: JoinSet<_> = {
            let _rt_guard = rt.enter();

            let mut tasks = JoinSet::new();

            // Start primary task
            info!("starting block-engine-primary");
            tasks.spawn(Self::start(
                Either::Left(block_engine_config.clone()),
                cluster_info.clone(),
                bundle_tx.clone(),
                packet_tx.clone(),
                banking_packet_sender.clone(),
                exit.clone(),
                block_builder_fee_info.clone(),
                shredstream_receiver_address.clone(),
                bam_enabled.clone(),
                input_tx_signature_sender.clone(),
                gui_metrics_sender.clone(),
                bundle_lifecycle_dump_enabled.clone(),
            ));

            // Start secondary URL manager task
            tasks.spawn(Self::manage_secondary_urls(
                secondary_entries.clone(),
                blocklisted_uuids.clone(),
                bank_forks,
                cluster_info.clone(),
                bundle_tx.clone(),
                packet_tx.clone(),
                banking_packet_sender.clone(),
                exit.clone(),
                block_builder_fee_info.clone(),
                secondary_task_exits.clone(),
                shredstream_receiver_address.clone(),
                bam_enabled.clone(),
                input_tx_signature_sender.clone(),
                gui_metrics_sender.clone(),
                bundle_lifecycle_dump_enabled,
                vote_account,
            ));

            tasks
        };

        let thread = Builder::new()
            .name("block-engine-runtime".to_string())
            .spawn(move || {
                rt.block_on(async move {
                    while let Some(res) = set.join_next().await {
                        match res {
                            Ok(_) => continue,
                            Err(e) => {
                                error!("Block engine task failed: {}", e);
                            }
                        }
                    }
                })
            })
            .unwrap();

        Self {
            t_hdls: vec![thread],
        }
    }

    pub fn join(self) -> thread::Result<()> {
        for t in self.t_hdls {
            t.join()?;
        }
        Ok(())
    }

    async fn manage_secondary_urls(
        secondary_entries: Arc<ArcSwap<Vec<BlockEngineEntry>>>,
        blocklisted_uuids: Arc<ArcSwap<Vec<String>>>,
        bank_forks: Arc<RwLock<BankForks>>,
        cluster_info: Arc<ClusterInfo>,
        bundle_tx: Sender<Vec<PacketBundle>>,
        packet_tx: Sender<PacketBatch>,
        banking_packet_sender: BankingPacketSender,
        exit: Arc<AtomicBool>,
        block_builder_fee_info: Arc<ArcSwap<BlockBuilderFeeInfo>>,
        secondary_task_exits: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
        shredstream_receiver_address: Arc<ArcSwap<Option<SocketAddr>>>,
        bam_enabled: Arc<AtomicU8>,
        input_tx_signature_sender: Option<(Sender<String>, Arc<AtomicBool>)>,
        gui_metrics_sender: Option<Sender<GuiCoreMetrics>>,
        bundle_lifecycle_dump_enabled: Arc<AtomicBool>,
        vote_account: Pubkey,
    ) {
        const CHECK_INTERVAL: Duration = Duration::from_secs(5);
        let mut check_interval = interval(CHECK_INTERVAL);
        let mut current_entries: Vec<BlockEngineEntry> = Vec::new();
        let mut task_set: JoinSet<()> = JoinSet::new();

        while !exit.load(Ordering::Relaxed) {
            tokio::select! {
                _ = check_interval.tick() => {
                    let admin_entries = secondary_entries.load().as_ref().clone();
                    let blocklist = blocklisted_uuids.load().as_ref().clone();
                    let onchain_entries = bank_forks.read().ok().and_then(|bank_forks_guard| {
                        load_secondary_block_engine_entries_from_bank(
                            &bank_forks_guard.working_bank(),
                            &vote_account,
                        )
                    });
                    let new_entries = merged_secondary_block_engine_entries(
                        &admin_entries,
                        onchain_entries,
                        &blocklist,
                    );

                    if new_entries != current_entries {
                        info!(
                            "Secondary block engine entries changed from {:#?} to {:#?}",
                            current_entries,
                            new_entries
                        );

                        let entries_to_remove: Vec<BlockEngineEntry> = current_entries
                            .iter()
                            .filter(|entry| !new_entries.contains(entry))
                            .cloned()
                            .collect();

                        let entries_to_add: Vec<BlockEngineEntry> = new_entries
                            .iter()
                            .filter(|entry| !current_entries.contains(entry))
                            .cloned()
                            .collect();

                        for entry in entries_to_remove {
                            if let Some(task_exit) = {
                                let mut exits = secondary_task_exits.lock().unwrap();
                                exits.remove(&secondary_task_key(&entry))
                            } {
                                info!(
                                    "Stopping task for removed block engine entry: uuid={}, url={}",
                                    entry.uuid, entry.url
                                );
                                task_exit.store(true, Ordering::Relaxed);
                            }
                        }

                        for entry in entries_to_add {
                            let task_exit = Arc::new(AtomicBool::new(false));

                            {
                                let mut exits = secondary_task_exits.lock().unwrap();
                                exits.insert(secondary_task_key(&entry), task_exit.clone());
                            }

                            info!(
                                "Starting task for new block engine entry: uuid={}, url={}, rate={:?}",
                                entry.uuid, entry.url, entry.bundle_rate_limit
                            );
                            task_set.spawn(Self::start(
                                Either::Right(entry.clone()),
                                cluster_info.clone(),
                                bundle_tx.clone(),
                                packet_tx.clone(),
                                banking_packet_sender.clone(),
                                task_exit,
                                block_builder_fee_info.clone(),
                                shredstream_receiver_address.clone(),
                                bam_enabled.clone(),
                                input_tx_signature_sender.clone(),
                                gui_metrics_sender.clone(),
                                bundle_lifecycle_dump_enabled.clone(),
                            ));
                        }

                        current_entries = new_entries;
                    }
                }
                Some(result) = task_set.join_next() => {
                    if let Err(e) = result {
                        error!("Secondary block engine task failed: {}", e);
                    }
                }
            }
        }

        {
            let exits = secondary_task_exits.lock().unwrap();
            for (uuid, task_exit) in exits.iter() {
                info!("Stopping secondary block engine task for uuid: {}", uuid);
                task_exit.store(true, Ordering::Relaxed);
            }
        }

        while task_set.join_next().await.is_some() {}
    }

    #[allow(clippy::too_many_arguments)]
    async fn start(
        block_engine_config: Either<Arc<ArcSwap<BlockEngineConfig>>, BlockEngineEntry>,
        cluster_info: Arc<ClusterInfo>,
        bundle_tx: Sender<Vec<PacketBundle>>,
        packet_tx: Sender<PacketBatch>,
        banking_packet_sender: BankingPacketSender,
        exit: Arc<AtomicBool>,
        block_builder_fee_info: Arc<ArcSwap<BlockBuilderFeeInfo>>,
        shredstream_receiver_address: Arc<ArcSwap<Option<SocketAddr>>>,
        bam_enabled: Arc<AtomicU8>,
        input_tx_signature_sender: Option<(Sender<String>, Arc<AtomicBool>)>,
        gui_metrics_sender: Option<Sender<GuiCoreMetrics>>,
        bundle_lifecycle_dump_enabled: Arc<AtomicBool>,
    ) {
        let mut error_count: u64 = 0;

        while !exit.load(Ordering::Relaxed) {
            // Wait until a valid config is supplied (either initially or by admin rpc)
            // Use if!/else here to avoid extra CONNECTION_BACKOFF wait on successful termination
            let local_block_engine_config = match &block_engine_config {
                Either::Left(config) => config.load().as_ref().clone(),
                Either::Right(entry) => BlockEngineConfig {
                    block_engine_url: entry.url.clone(),
                    block_engine_uuid: entry.uuid.clone(),
                    disable_block_engine_autoconfig: false,
                    trust_packets: false, // Default to false for secondary URLs
                    bundle_rate_limit: entry.bundle_rate_limit.clone(),
                },
            };
            if !Self::is_valid_block_engine_config(&local_block_engine_config) {
                Self::maybe_clear_shredstream_receiver_address(
                    &block_engine_config,
                    &shredstream_receiver_address,
                );
                sleep(Self::CONNECTION_BACKOFF).await;
                continue;
            }

            if let Err(e) = Self::connect_auth_and_stream_maybe_autoconfig(
                &block_engine_config,
                &cluster_info,
                &bundle_tx,
                &packet_tx,
                &banking_packet_sender,
                &exit,
                &block_builder_fee_info,
                &shredstream_receiver_address,
                &local_block_engine_config,
                &bam_enabled,
                &input_tx_signature_sender,
                &gui_metrics_sender,
                &bundle_lifecycle_dump_enabled,
            )
            .await
            {
                match e {
                    // This error is frequent on hot spares, and the parsed string does not work
                    // with datapoints (incorrect escaping).
                    ProxyError::AuthenticationPermissionDenied => warn!(
                        "block engine permission denied. not on leader schedule. ignore if \
                         hot-spare."
                    ),
                    ProxyError::BamEnabled => {}
                    e => {
                        error_count += 1;
                        datapoint_warn!(
                            "block_engine_stage-proxy_error",
                            ("count", error_count, i64),
                            ("error", e.to_string(), String),
                            (
                                "url",
                                local_block_engine_config.block_engine_url.clone(),
                                String
                            ),
                            ("is_primary", block_engine_config.is_left(), bool),
                        );
                    }
                }
                sleep(Self::CONNECTION_BACKOFF).await;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn connect_auth_and_stream_maybe_autoconfig(
        block_engine_config: &Either<Arc<ArcSwap<BlockEngineConfig>>, BlockEngineEntry>,
        cluster_info: &Arc<ClusterInfo>,
        bundle_tx: &Sender<Vec<PacketBundle>>,
        packet_tx: &Sender<PacketBatch>,
        banking_packet_sender: &BankingPacketSender,
        exit: &Arc<AtomicBool>,
        block_builder_fee_info: &Arc<ArcSwap<BlockBuilderFeeInfo>>,
        shredstream_receiver_address: &Arc<ArcSwap<Option<SocketAddr>>>,
        local_block_engine_config: &BlockEngineConfig,
        bam_enabled: &Arc<AtomicU8>,
        input_tx_signature_sender: &Option<(Sender<String>, Arc<AtomicBool>)>,
        gui_metrics_sender: &Option<Sender<GuiCoreMetrics>>,
        bundle_lifecycle_dump_enabled: &Arc<AtomicBool>,
    ) -> crate::proxy::Result<()> {
        if BamConnectionState::from_u8(bam_enabled.load(Ordering::Relaxed))
            == BamConnectionState::Connected
        {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            return Ok(());
        }

        let endpoint = Self::get_endpoint(&local_block_engine_config.block_engine_url)?;
        if !local_block_engine_config.disable_block_engine_autoconfig {
            datapoint_info!(
                "block_engine_stage-connect",
                "type" => "autoconfig",
                ("count", 1, i64),
            );
            return Self::connect_auth_and_stream_autoconfig(
                endpoint,
                local_block_engine_config,
                block_engine_config,
                cluster_info,
                bundle_tx,
                packet_tx,
                banking_packet_sender,
                exit,
                block_builder_fee_info,
                shredstream_receiver_address,
                bam_enabled,
                input_tx_signature_sender,
                gui_metrics_sender,
                bundle_lifecycle_dump_enabled,
            )
            .await
            .map_err(|err| Self::map_bam_enabled(bam_enabled, err));
        }

        let Some(global) = Self::get_block_engine_endpoints(&endpoint)
            .await
            .map_err(|err| Self::map_bam_enabled(bam_enabled, err))?
            .global_endpoint
        else {
            return Err(Self::map_bam_enabled(
                bam_enabled,
                ProxyError::BlockEngineEndpointError(
                    "Block engine configuration failed: no global endpoint found".to_owned(),
                ),
            ));
        };

        datapoint_info!(
            "block_engine_stage-connect",
            "type" => "direct_global",
            ("count", 1, i64),
        );
        Self::maybe_update_shredstream_receiver_address(
            block_engine_config,
            shredstream_receiver_address,
            Self::resolve_shredstream_receiver_address(&global.shredstream_receiver_address),
        );
        let backend_endpoint = Self::get_endpoint(global.block_engine_url.as_str())?;

        datapoint_info!(
            "block_engine_stage-connect",
            "type" => "direct",
            ("count", 1, i64),
        );
        Self::connect_auth_and_stream(
            &backend_endpoint,
            local_block_engine_config,
            block_engine_config,
            cluster_info,
            bundle_tx,
            packet_tx,
            banking_packet_sender,
            exit,
            block_builder_fee_info,
            &Self::CONNECTION_TIMEOUT,
            bam_enabled,
            input_tx_signature_sender,
            gui_metrics_sender,
            bundle_lifecycle_dump_enabled,
        )
        .await
        .map_err(|err| Self::map_bam_enabled(bam_enabled, err))
        .inspect(|_| {
            datapoint_info!(
                "block_engine_stage-connect",
                "type" => "closed_connection",
                ("url", endpoint.uri().to_string(), String),
                ("count", 1, i64),
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn connect_auth_and_stream_autoconfig(
        endpoint: Endpoint,
        local_block_engine_config: &BlockEngineConfig,
        global_block_engine_config: &Either<Arc<ArcSwap<BlockEngineConfig>>, BlockEngineEntry>,
        cluster_info: &Arc<ClusterInfo>,
        bundle_tx: &Sender<Vec<PacketBundle>>,
        packet_tx: &Sender<PacketBatch>,
        banking_packet_sender: &BankingPacketSender,
        exit: &Arc<AtomicBool>,
        block_builder_fee_info: &Arc<ArcSwap<BlockBuilderFeeInfo>>,
        shredstream_receiver_address: &Arc<ArcSwap<Option<SocketAddr>>>,
        bam_enabled: &Arc<AtomicU8>,
        input_tx_signature_sender: &Option<(Sender<String>, Arc<AtomicBool>)>,
        gui_metrics_sender: &Option<Sender<GuiCoreMetrics>>,
        bundle_lifecycle_dump_enabled: &Arc<AtomicBool>,
    ) -> crate::proxy::Result<()> {
        let endpoints = Self::get_block_engine_endpoints(&endpoint)
            .await
            .map_err(|err| Self::map_bam_enabled(bam_enabled, err))?;
        datapoint_info!(
            "block_engine_stage-autoconfig",
            ("regioned_count", endpoints.regioned_endpoints.len(), i64),
            ("count", 1, i64),
        );
        let candidates = Self::probe_and_rank_endpoints(&endpoints.regioned_endpoints).await;
        let candidates = if candidates.is_empty() {
            let Some(global) = endpoints.global_endpoint else {
                return Err(Self::map_bam_enabled(
                    bam_enabled,
                    ProxyError::BlockEngineEndpointError(
                        "Block engine configuration failed: no reachable endpoints found"
                            .to_owned(),
                    ),
                ));
            };

            ahash::HashMap::from_iter([(
                global.block_engine_url,
                (
                    Self::resolve_shredstream_receiver_address(
                        &global.shredstream_receiver_address,
                    ),
                    u64::MAX,
                ),
            )])
        } else {
            candidates
        };

        // try connecting to best block engine
        let mut attempted = false;
        let mut backend_endpoint = endpoint.clone();
        let endpoint_count = candidates.len();
        for (block_engine_url, (maybe_shredstream_socket, latency_us)) in candidates
            .into_iter()
            .sorted_unstable_by_key(|(_endpoint, (_shredstream_socket, latency_us))| *latency_us)
        {
            if block_engine_url != local_block_engine_config.block_engine_url {
                info!(
                    "Selected best Block Engine url: {block_engine_url}, Shredstream socket: \
                     {maybe_shredstream_socket:?}, rtt: ({:?})",
                    Duration::from_micros(latency_us)
                );
                backend_endpoint = Self::get_endpoint(block_engine_url.as_str())?;
            }
            Self::maybe_update_shredstream_receiver_address(
                global_block_engine_config,
                shredstream_receiver_address,
                maybe_shredstream_socket,
            );
            attempted = true;
            let connect_start = Instant::now();
            match Self::connect_auth_and_stream(
                &backend_endpoint,
                local_block_engine_config,
                global_block_engine_config,
                cluster_info,
                bundle_tx,
                packet_tx,
                banking_packet_sender,
                exit,
                block_builder_fee_info,
                &Self::CONNECTION_TIMEOUT,
                bam_enabled,
                input_tx_signature_sender,
                gui_metrics_sender,
                bundle_lifecycle_dump_enabled,
            )
            .await
            .map_err(|err| Self::map_bam_enabled(bam_enabled, err))
            {
                Ok(()) => {
                    datapoint_info!(
                        "block_engine_stage-connect",
                        "type" => "closed_connection",
                        ("url", backend_endpoint.uri().to_string(), String),
                        ("count", 1, i64),
                    );
                    return Ok(());
                }
                Err(e) => {
                    // log each connection error
                    match &e {
                        // This error is frequent on hot spares, and the parsed string does not work
                        // with datapoints (incorrect escaping).
                        ProxyError::AuthenticationPermissionDenied => warn!(
                            "block engine permission denied. not on leader schedule. ignore if \
                             hot-spare."
                        ),
                        ProxyError::BamEnabled => return Ok(()),
                        other => {
                            datapoint_warn!(
                                "block_engine_stage-autoconfig_error",
                                "type" => "proxy_err",
                                ("url", block_engine_url, String),
                                ("count", 1, i64),
                                ("error", other.to_string(), String),
                            );
                        }
                    }

                    if connect_start.elapsed() > Self::CONNECTION_TIMEOUT * 3 {
                        return Err(e); // run a new round of probes and connect to new best
                    }
                    // Otherwise, try next endpoint without delay; caller handles backoff on overall failure
                }
            }
        }
        if !attempted {
            return Err(ProxyError::BlockEngineEndpointError(
                "autoconfig failed: no endpoints available after gRPC RTT ranking".to_string(),
            ));
        }
        Err(ProxyError::BlockEngineEndpointError(format!(
            "autoconfig failed: all {endpoint_count} candidate endpoints failed to connect",
        )))
    }

    fn map_bam_enabled(bam_enabled: &Arc<AtomicU8>, err: ProxyError) -> ProxyError {
        match BamConnectionState::from_u8(bam_enabled.load(Ordering::Relaxed)) {
            BamConnectionState::Disconnected => err,
            BamConnectionState::Connecting | BamConnectionState::Connected => {
                ProxyError::BamEnabled
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn connect_auth_and_stream(
        backend_endpoint: &Endpoint,
        local_block_engine_config: &BlockEngineConfig,
        global_block_engine_config: &Either<Arc<ArcSwap<BlockEngineConfig>>, BlockEngineEntry>,
        cluster_info: &Arc<ClusterInfo>,
        bundle_tx: &Sender<Vec<PacketBundle>>,
        packet_tx: &Sender<PacketBatch>,
        banking_packet_sender: &BankingPacketSender,
        exit: &Arc<AtomicBool>,
        block_builder_fee_info: &Arc<ArcSwap<BlockBuilderFeeInfo>>,
        connection_timeout: &Duration,
        bam_enabled: &Arc<AtomicU8>,
        input_tx_signature_sender: &Option<(Sender<String>, Arc<AtomicBool>)>,
        gui_metrics_sender: &Option<Sender<GuiCoreMetrics>>,
        bundle_lifecycle_dump_enabled: &Arc<AtomicBool>,
    ) -> crate::proxy::Result<()> {
        // Get a copy of configs here in case they have changed at runtime
        let keypair = cluster_info.keypair().clone();

        debug!("connecting to auth: {}", backend_endpoint.uri());
        let (auth_client, access_token, refresh_token) =
            auth_client_from_endpoint(backend_endpoint, connection_timeout, keypair.as_ref())
                .await
                .map_err(|err| Self::map_bam_enabled(bam_enabled, err))?;

        let backend_url = backend_endpoint.uri().to_string();
        datapoint_info!(
            "block_engine_stage-tokens_generated",
            ("url", local_block_engine_config.block_engine_url, String),
            ("is_primary", global_block_engine_config.is_left(), bool),
            ("count", 1, i64),
        );

        debug!("connecting to block engine: {}", backend_endpoint.uri());
        let block_engine_channel = timeout(*connection_timeout, backend_endpoint.connect())
            .await
            .map_err(|_| ProxyError::BlockEngineConnectionTimeout)
            .map_err(|err| Self::map_bam_enabled(bam_enabled, err))?
            .map_err(|err| {
                Self::map_bam_enabled(
                    bam_enabled,
                    ProxyError::BlockEngineConnectionError(Box::new(err)),
                )
            })?;
        let block_engine_client = BlockEngineValidatorClient::with_interceptor(
            block_engine_channel,
            AuthInterceptor::new(access_token.clone()),
        );
        datapoint_info!(
            "block_engine_stage-connected",
            ("url", backend_url, String),
            ("count", 1, i64),
        );

        Self::start_consuming_block_engine_bundles_and_packets(
            bundle_tx,
            block_engine_client,
            packet_tx,
            local_block_engine_config,
            global_block_engine_config,
            banking_packet_sender,
            exit,
            block_builder_fee_info,
            auth_client,
            access_token,
            refresh_token,
            connection_timeout,
            keypair,
            cluster_info,
            bam_enabled,
            input_tx_signature_sender,
            gui_metrics_sender,
            bundle_lifecycle_dump_enabled,
        )
        .await
    }

    /// Build an Endpoint from the URL provided
    fn get_endpoint(block_engine_url: &str) -> Result<Endpoint, ProxyError> {
        endpoint_from_url(
            block_engine_url,
            || {
                ProxyError::BlockEngineEndpointError(format!(
                    "invalid block engine url value: {block_engine_url}",
                ))
            },
            || {
                ProxyError::BlockEngineEndpointError(format!(
                    "failed to set tls_config for block engine: {block_engine_url}",
                ))
            },
        )
    }

    async fn probe_grpc_rtt_us(block_engine_url: &str) -> Result<u64, ProbeError> {
        const PROBE_COUNT: usize = 3;

        // Connect once and probe multiple times so we're not ranking on handshake costs.
        let endpoint = Self::get_endpoint(block_engine_url)?;
        let channel = timeout(Self::CONNECTION_TIMEOUT, endpoint.connect())
            .await
            .map_err(|_| ProbeError::ConnectTimeout)??;

        let mut client = BlockEngineValidatorClient::new(channel);

        let mut best_us: u64 = u64::MAX;
        let mut any_success = false;
        for sample in 0..PROBE_COUNT {
            let start = Instant::now();
            let res = timeout(
                Self::CONNECTION_TIMEOUT,
                client.get_block_engine_endpoints(GetBlockEngineEndpointRequest {}),
            )
            .await;
            match res {
                Ok(Ok(_resp)) => {
                    let elapsed_us = start.elapsed().as_micros() as u64;
                    any_success = true;
                    best_us = best_us.min(elapsed_us);
                    datapoint_info!(
                        "block_engine_stage-autoconfig_ping",
                        "method" => "grpc",
                        ("endpoint", block_engine_url, String),
                        ("latency_us", elapsed_us, i64),
                        ("sample", sample, i64),
                    );
                }
                Ok(Err(status)) => {
                    datapoint_warn!(
                        "block_engine_stage-autoconfig_error",
                        "type" => "probe_request",
                        ("url", block_engine_url, String),
                        ("count", 1, i64),
                        ("err", status.to_string(), String),
                    );
                }
                Err(_elapsed) => {
                    datapoint_warn!(
                        "block_engine_stage-autoconfig_error",
                        "type" => "probe_timeout",
                        ("url", block_engine_url, String),
                        ("count", 1, i64),
                        ("err", "timeout", String),
                    );
                }
            }
        }

        if any_success {
            Ok(best_us)
        } else {
            Err(ProbeError::NoSuccessfulSamples)
        }
    }

    /// Probe all candidate endpoints concurrently, aggregate best RTT per endpoint.
    async fn probe_and_rank_endpoints(
        endpoints: &[BlockEngineEndpoint],
    ) -> ahash::HashMap<
        String, /* block engine url */
        (
            Option<SocketAddr>, /* shredstream receiver, fallable when DNS can't resolve */
            u64,                /* latency us */
        ),
    > {
        let mut agg_endpoints: ahash::HashMap<
            String, /* block engine url */
            (
                Option<SocketAddr>, /* shredstream receiver, fallable when DNS can't resolve */
                u64,                /* latency us */
            ),
        > = ahash::HashMap::with_capacity(endpoints.len());
        let mut best_endpoint_url = String::new();
        let mut best_endpoint_rtt_us = u64::MAX;

        let tasks = endpoints
            .iter()
            .map(|endpoint| {
                let endpoint = endpoint.clone();
                task::spawn(async move {
                    let rtt_res = Self::probe_grpc_rtt_us(&endpoint.block_engine_url).await;
                    (endpoint, rtt_res)
                })
            })
            .collect_vec();

        for join_res in futures::future::join_all(tasks).await {
            let (endpoint, rtt_res) = match join_res {
                Ok(v) => v,
                Err(e) => {
                    datapoint_warn!(
                        "block_engine_stage-autoconfig_error",
                        "type" => "probe_join",
                        ("count", 1, i64),
                        ("err", e.to_string(), String),
                    );
                    continue;
                }
            };

            let rtt_us = match rtt_res {
                Ok(v) => v,
                Err(e) => {
                    datapoint_warn!(
                        "block_engine_stage-autoconfig_error",
                        "type" => "probe",
                        ("url", endpoint.block_engine_url.as_str(), String),
                        ("count", 1, i64),
                        ("err", e.to_string(), String),
                    );
                    continue;
                }
            };

            if rtt_us <= best_endpoint_rtt_us {
                best_endpoint_rtt_us = rtt_us;
                best_endpoint_url = endpoint.block_engine_url.clone();
            }

            match agg_endpoints.entry(endpoint.block_engine_url.clone()) {
                Entry::Occupied(mut ent) => {
                    let (_shredstream_socket, best_rtt_us) = ent.get_mut();
                    if rtt_us <= *best_rtt_us {
                        *best_rtt_us = rtt_us;
                    }
                }
                Entry::Vacant(entry) => {
                    let maybe_shredstream_socket = endpoint
                        .shredstream_receiver_address
                        .to_socket_addrs()
                        .inspect_err(|e| {
                            datapoint_warn!(
                                "block_engine_stage-autoconfig_error",
                                "type" => "shredstream_resolve",
                                ("address", endpoint.block_engine_url.as_str(), String),
                                ("count", 1, i64),
                                ("err", e.to_string(), String),
                            );
                        })
                        .ok()
                        .and_then(|mut shredstream_sockets| shredstream_sockets.next());
                    entry.insert((maybe_shredstream_socket, rtt_us));
                }
            };
        }

        datapoint_info!(
            "block_engine_stage-autoconfig",
            ("endpoints_count", agg_endpoints.len(), i64),
            ("best_endpoint_url", best_endpoint_url.as_str(), String),
            ("best_endpoint_latency_us", best_endpoint_rtt_us, i64),
            ("count", 1, i64),
        );

        agg_endpoints
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_consuming_block_engine_bundles_and_packets(
        bundle_tx: &Sender<Vec<PacketBundle>>,
        mut client: BlockEngineValidatorClient<InterceptedService<Channel, AuthInterceptor>>,
        packet_tx: &Sender<PacketBatch>,
        local_config: &BlockEngineConfig, // local copy of config with current connections
        global_config: &Either<Arc<ArcSwap<BlockEngineConfig>>, BlockEngineEntry>, // guarded reference for detecting run-time updates
        banking_packet_sender: &BankingPacketSender,
        exit: &Arc<AtomicBool>,
        block_builder_fee_info: &Arc<ArcSwap<BlockBuilderFeeInfo>>,
        auth_client: AuthServiceClient<Channel>,
        access_token: Arc<ArcSwap<Token>>,
        refresh_token: Token,
        connection_timeout: &Duration,
        keypair: Arc<Keypair>,
        cluster_info: &Arc<ClusterInfo>,
        bam_enabled: &Arc<AtomicU8>,
        input_tx_signature_sender: &Option<(Sender<String>, Arc<AtomicBool>)>,
        gui_metrics_sender: &Option<Sender<GuiCoreMetrics>>,
        bundle_lifecycle_dump_enabled: &Arc<AtomicBool>,
    ) -> crate::proxy::Result<()> {
        let subscribe_packets_stream = timeout(
            *connection_timeout,
            client.subscribe_packets(block_engine::SubscribePacketsRequest {}),
        )
        .await
        .map_err(|_| ProxyError::MethodTimeout("block_engine_subscribe_packets".to_string()))
        .map_err(|err| Self::map_bam_enabled(bam_enabled, err))?
        .map_err(|status| {
            Self::map_bam_enabled(
                bam_enabled,
                ProxyError::MethodError {
                    code: status.code(),
                    message: sanitize_status_message_for_influx(status.message()),
                },
            )
        })?
        .into_inner();

        let subscribe_bundles_stream = timeout(
            *connection_timeout,
            client.subscribe_bundles(block_engine::SubscribeBundlesRequest {}),
        )
        .await
        .map_err(|_| ProxyError::MethodTimeout("subscribe_bundles".to_string()))
        .map_err(|err| Self::map_bam_enabled(bam_enabled, err))?
        .map_err(|status| {
            Self::map_bam_enabled(
                bam_enabled,
                ProxyError::MethodError {
                    code: status.code(),
                    message: sanitize_status_message_for_influx(status.message()),
                },
            )
        })?
        .into_inner();

        // Only update block builder fee info for primary connections
        if global_config.is_left() {
            Self::refresh_block_builder_fee_info(
                &mut client,
                connection_timeout,
                block_builder_fee_info,
                &local_config.block_engine_url,
                bam_enabled,
            )
            .await?;
        }

        Self::consume_bundle_and_packet_stream(
            client,
            (subscribe_bundles_stream, subscribe_packets_stream),
            bundle_tx,
            packet_tx,
            local_config,
            global_config,
            banking_packet_sender,
            exit,
            block_builder_fee_info,
            auth_client,
            access_token,
            refresh_token,
            keypair,
            cluster_info,
            connection_timeout,
            bam_enabled,
            input_tx_signature_sender,
            gui_metrics_sender,
            bundle_lifecycle_dump_enabled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn consume_bundle_and_packet_stream(
        mut client: BlockEngineValidatorClient<InterceptedService<Channel, AuthInterceptor>>,
        (mut bundle_stream, mut packet_stream): (
            Streaming<block_engine::SubscribeBundlesResponse>,
            Streaming<block_engine::SubscribePacketsResponse>,
        ),
        bundle_tx: &Sender<Vec<PacketBundle>>,
        packet_tx: &Sender<PacketBatch>,
        local_config: &BlockEngineConfig, // local copy of config with current connections
        global_config: &Either<Arc<ArcSwap<BlockEngineConfig>>, BlockEngineEntry>, // guarded reference for detecting run-time updates
        banking_packet_sender: &BankingPacketSender,
        exit: &Arc<AtomicBool>,
        block_builder_fee_info: &Arc<ArcSwap<BlockBuilderFeeInfo>>,
        mut auth_client: AuthServiceClient<Channel>,
        access_token: Arc<ArcSwap<Token>>,
        mut refresh_token: Token,
        keypair: Arc<Keypair>,
        cluster_info: &Arc<ClusterInfo>,
        connection_timeout: &Duration,
        #[allow(unused_variables)] bam_enabled: &Arc<AtomicU8>,
        input_tx_signature_sender: &Option<(Sender<String>, Arc<AtomicBool>)>,
        gui_metrics_sender: &Option<Sender<GuiCoreMetrics>>,
        bundle_lifecycle_dump_enabled: &Arc<AtomicBool>,
    ) -> crate::proxy::Result<()> {
        const METRICS_TICK: Duration = Duration::from_secs(1);
        const MAINTENANCE_TICK: Duration = Duration::from_secs(10 * 60);
        let refresh_within_s: u64 = METRICS_TICK.as_secs().saturating_mul(3).saturating_div(2);

        let metrics_report_tick = if gui_metrics_sender.is_some() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(1000)
        };

        let mut num_full_refreshes: u64 = 1;
        let mut num_refresh_access_token: u64 = 0;
        let mut block_engine_stats = BlockEngineStageStats::default();
        let mut metrics_and_auth_tick = interval(metrics_report_tick);
        let mut maintenance_tick = interval(MAINTENANCE_TICK);
        let bundle_limiter = if global_config.is_left() {
            None
        } else {
            maybe_bundle_limiter(&local_config.bundle_rate_limit)
        };

        info!(
            "connected to packet and bundle stream: {} (primary: {})",
            local_config.block_engine_url,
            global_config.is_left()
        );

        // Per-connection BE peer IPv4 (primary and each secondary resolve independently).
        // Used as FD-style fallback when bundle packet meta.addr is not a usable IPv4.
        let block_engine_ipv4 = Self::resolve_block_engine_ipv4(&local_config.block_engine_url);
        if let Some(ip) = block_engine_ipv4 {
            info!(
                "block engine source ipv4 fallback for {}: {ip}",
                local_config.block_engine_url
            );
        } else {
            warn!(
                "failed to resolve block engine ipv4 for {}; bundle txn ips may be 0.0.0.0",
                local_config.block_engine_url
            );
        }

        while !exit.load(Ordering::Relaxed) {
            if BamConnectionState::from_u8(bam_enabled.load(Ordering::Relaxed))
                == BamConnectionState::Connected
            {
                info!("bam enabled, exiting block engine stage");
                return Ok(());
            }

            tokio::select! {
                maybe_packet = packet_stream.message() => {
                    let resp = maybe_packet
                        .map_err(ProxyError::from)?
                        .ok_or(ProxyError::GrpcStreamDisconnected)
                        .map_err(|err| Self::map_bam_enabled(bam_enabled, err))?;
                    Self::handle_block_engine_packets(resp, packet_tx, banking_packet_sender, local_config.trust_packets, &mut block_engine_stats, input_tx_signature_sender)?;
                }
                maybe_bundles = bundle_stream.message() => {
                    let resp = maybe_bundles
                        .map_err(ProxyError::from)?
                        .ok_or(ProxyError::GrpcStreamDisconnected)
                        .map_err(|err| Self::map_bam_enabled(bam_enabled, err))?;
                    Self::handle_block_engine_bundles(
                        resp,
                        bundle_tx,
                        &local_config.block_engine_uuid,
                        block_engine_ipv4,
                        global_config.is_left(),
                        bundle_limiter.as_deref(),
                        bundle_lifecycle_dump_enabled.load(Ordering::Relaxed),
                        &mut block_engine_stats,
                    )?;
                }
                _ = metrics_and_auth_tick.tick() => {
                    if let Some(gui_metrics_sender) = gui_metrics_sender {
                        if let Err(err) = gui_metrics_sender.try_send(GuiCoreMetrics::BlockEngine(block_engine_stats.clone())) {
                            warn!("failed to send BlockEngine gui metrics: {err}");
                        }
                    }
                    block_engine_stats.report_with_url(&local_config.block_engine_url, global_config.is_left());
                    block_engine_stats = BlockEngineStageStats::default();

                    if cluster_info.id() != keypair.pubkey() {
                        return Err(ProxyError::AuthenticationConnectionError("validator identity changed".to_string()));
                    }

                    // Only check config changes for primary connection
                    if let Either::Left(global_config) = global_config {
                        if global_config.load().as_ref() != local_config {
                            return Err(ProxyError::BlockEngineConfigChanged);
                        }
                    }

                    let (maybe_new_access, maybe_new_refresh) = maybe_refresh_auth_tokens(&mut auth_client,
                        &access_token,
                        &refresh_token,
                        cluster_info,
                        connection_timeout,
                        refresh_within_s,
                    ).await
                    .map_err(|err| Self::map_bam_enabled(bam_enabled, err))?;

                    if let Some(new_token) = maybe_new_access {
                        num_refresh_access_token += 1;
                        datapoint_info!(
                            "block_engine_stage-refresh_access_token",
                            ("url", &local_config.block_engine_url, String),
                            ("is_primary", global_config.is_left(), bool),
                            ("count", num_refresh_access_token, i64),
                        );

                        access_token.store(Arc::new(new_token));
                    }
                    if let Some(new_token) = maybe_new_refresh {
                        num_full_refreshes += 1;
                        datapoint_info!(
                            "block_engine_stage-tokens_generated",
                            ("url", &local_config.block_engine_url, String),
                            ("is_primary", global_config.is_left(), bool),

                            ("count", num_full_refreshes, i64),
                        );
                        refresh_token = new_token;
                    }
                }
                // Only update fee info periodically for primary connection
                _ = maintenance_tick.tick(), if global_config.is_left() => {
                    Self::refresh_block_builder_fee_info(
                        &mut client,
                        connection_timeout,
                        block_builder_fee_info,
                        &local_config.block_engine_url,
                        bam_enabled,
                    )
                    .await?;
                }
            }
        }

        Ok(())
    }

    fn handle_block_engine_bundles(
        bundles_response: block_engine::SubscribeBundlesResponse,
        bundle_sender: &Sender<Vec<PacketBundle>>,
        block_engine_uuid: &str,
        block_engine_ipv4: Option<Ipv4Addr>,
        is_primary: bool,
        limiter: Option<&DefaultDirectRateLimiter>,
        dump_enabled: bool,
        block_engine_stats: &mut BlockEngineStageStats,
    ) -> crate::proxy::Result<()> {
        let bundles = admit_bundles_with_rate_limit(
            bundles_response,
            block_engine_uuid,
            is_primary,
            limiter,
            dump_enabled,
            block_engine_stats,
            block_engine_ipv4,
        );
        if bundles.is_empty() {
            return Ok(());
        }
        // NOTE: bundles are sanitized in bundle_sanitizer module
        bundle_sender
            .send(bundles)
            .map_err(|_| ProxyError::PacketForwardError)
    }

    /// FD-style: keep a usable packet IPv4; otherwise stamp this connection's BE peer IP.
    fn apply_bundle_source_ipv4_fallback(
        packet: &mut BytesPacket,
        block_engine_ipv4: Option<Ipv4Addr>,
    ) {
        let Some(fallback) = block_engine_ipv4 else {
            return;
        };
        let needs_fallback = match packet.meta().addr {
            IpAddr::V4(v4) => v4.is_unspecified(),
            IpAddr::V6(_) => true,
        };
        if needs_fallback {
            packet.meta_mut().addr = IpAddr::V4(fallback);
        }
    }

    /// Resolve this connection's block-engine host to an IPv4 once (no per-bundle DNS).
    fn resolve_block_engine_ipv4(block_engine_url: &str) -> Option<Ipv4Addr> {
        let without_scheme = block_engine_url
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(block_engine_url);
        let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
        if host_port.starts_with('[') {
            // IPv6 authority — GUI source field is IPv4-only.
            return None;
        }
        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port_str)) if !host.is_empty() && port_str.parse::<u16>().is_ok() => {
                (host, port_str.parse::<u16>().ok()?)
            }
            _ => (host_port, 443),
        };
        if let Ok(IpAddr::V4(v4)) = host.parse() {
            return Some(v4);
        }
        (host, port)
            .to_socket_addrs()
            .ok()?
            .find_map(|addr| match addr.ip() {
                IpAddr::V4(v4) => Some(v4),
                IpAddr::V6(_) => None,
            })
    }

    fn handle_block_engine_packets(
        resp: block_engine::SubscribePacketsResponse,
        packet_tx: &Sender<PacketBatch>,
        banking_packet_sender: &BankingPacketSender,
        trust_packets: bool,
        block_engine_stats: &mut BlockEngineStageStats,
        input_tx_signature_sender: &Option<(Sender<String>, Arc<AtomicBool>)>,
    ) -> crate::proxy::Result<()> {
        if let Some(batch) = resp.batch {
            if batch.packets.is_empty() {
                block_engine_stats.num_empty_packets.add_assign(1);
                return Ok(());
            }

            let packet_batch = PacketBatch::from(
                batch
                    .packets
                    .into_iter()
                    .map(proto_packet_to_packet)
                    .collect::<Vec<BytesPacket>>(),
            );

            block_engine_stats
                .num_packets
                .add_assign(packet_batch.len() as u64);

            if trust_packets {
                banking_packet_sender
                    .send(Arc::new(vec![packet_batch]), input_tx_signature_sender)
                    .map_err(|_| ProxyError::PacketForwardError)?;
            } else {
                packet_tx
                    .send(packet_batch)
                    .map_err(|_| ProxyError::PacketForwardError)?;
            }
        } else {
            block_engine_stats.num_empty_packets.add_assign(1);
        }

        Ok(())
    }

    pub fn is_valid_block_engine_config(config: &BlockEngineConfig) -> bool {
        if config.block_engine_url.is_empty() {
            warn!("can't connect to block_engine. missing block_engine_url.");
            return false;
        }
        if let Err(e) = Self::get_endpoint(&config.block_engine_url) {
            error!("can't connect to block engine. error creating block engine endpoint - {e}");
            return false;
        }
        true
    }

    async fn get_block_engine_endpoints(
        backend_endpoint: &Endpoint,
    ) -> crate::proxy::Result<block_engine::GetBlockEngineEndpointResponse> {
        BlockEngineValidatorClient::connect(backend_endpoint.clone())
            .await
            .map_err(|e| ProxyError::BlockEngineConnectionError(Box::new(e)))?
            .get_block_engine_endpoints(GetBlockEngineEndpointRequest {})
            .await
            .map_err(|s| ProxyError::BlockEngineRequestError {
                code: s.code(),
                message: sanitize_status_message_for_influx(s.message()),
            })
            .map(|response| response.into_inner())
    }

    fn maybe_update_shredstream_receiver_address(
        block_engine_config: &Either<Arc<ArcSwap<BlockEngineConfig>>, BlockEngineEntry>,
        shredstream_receiver_address: &Arc<ArcSwap<Option<SocketAddr>>>,
        maybe_shredstream_socket: Option<SocketAddr>,
    ) {
        if block_engine_config.is_left() {
            if let Some(shredstream_socket) = maybe_shredstream_socket {
                shredstream_receiver_address.store(Arc::new(Some(shredstream_socket)));
            }
        }
    }

    fn maybe_clear_shredstream_receiver_address(
        block_engine_config: &Either<Arc<ArcSwap<BlockEngineConfig>>, BlockEngineEntry>,
        shredstream_receiver_address: &Arc<ArcSwap<Option<SocketAddr>>>,
    ) {
        if block_engine_config.is_left() {
            shredstream_receiver_address.store(Arc::new(None));
        }
    }

    fn resolve_shredstream_receiver_address(address: &str) -> Option<SocketAddr> {
        address
            .to_socket_addrs()
            .inspect_err(|e| {
                datapoint_warn!(
                    "block_engine_stage-autoconfig_error",
                    "type" => "shredstream_resolve",
                    ("address", address, String),
                    ("count", 1, i64),
                    ("err", e.to_string(), String),
                );
            })
            .ok()
            .and_then(|mut shredstream_sockets| shredstream_sockets.next())
    }

    async fn refresh_block_builder_fee_info(
        client: &mut BlockEngineValidatorClient<InterceptedService<Channel, AuthInterceptor>>,
        connection_timeout: &Duration,
        block_builder_fee_info: &Arc<ArcSwap<BlockBuilderFeeInfo>>,
        block_engine_url: &str,
        bam_enabled: &Arc<AtomicU8>,
    ) -> crate::proxy::Result<()> {
        let block_builder_info = timeout(
            *connection_timeout,
            client.get_block_builder_fee_info(BlockBuilderFeeInfoRequest {}),
        )
        .await
        .map_err(|_| ProxyError::MethodTimeout("get_block_builder_fee_info".to_string()))
        .map_err(|err| Self::map_bam_enabled(bam_enabled, err))?
        .map_err(|status| {
            Self::map_bam_enabled(
                bam_enabled,
                ProxyError::MethodError {
                    code: status.code(),
                    message: sanitize_status_message_for_influx(status.message()),
                },
            )
        })?
        .into_inner();
        let block_builder_pubkey =
            Pubkey::from_str(&block_builder_info.pubkey).unwrap_or_else(|_| {
                datapoint_warn!(
                    "block_engine_stage-block_builder_pubkey_parse_error",
                    ("url", block_engine_url, String),
                    ("pubkey", &block_builder_info.pubkey, String),
                    ("count", 1, i64),
                );
                block_builder_fee_info.load().block_builder
            });
        block_builder_fee_info.store(Arc::new(BlockBuilderFeeInfo {
            block_builder: block_builder_pubkey,
            block_builder_commission: block_builder_info.commission,
        }));
        Ok(())
    }
}

fn admit_bundles_with_rate_limit(
    bundles_response: block_engine::SubscribeBundlesResponse,
    block_engine_uuid: &str,
    is_primary: bool,
    limiter: Option<&DefaultDirectRateLimiter>,
    dump_enabled: bool,
    block_engine_stats: &mut BlockEngineStageStats,
    block_engine_ipv4: Option<Ipv4Addr>,
) -> Vec<PacketBundle> {
    let mut bundle_packets = 0u64;
    let bundles: Vec<PacketBundle> = bundles_response
        .bundles
        .into_iter()
        .filter_map(|bundle| {
            if limiter.is_some_and(|limiter| limiter.check().is_err()) {
                block_engine_stats.num_bundles_throttled.add_assign(1);
                if dump_enabled {
                    BundleExecutionStats::report_immediate_drop(
                        &bundle.uuid,
                        block_engine_uuid.to_string(),
                        is_primary,
                        BundleDropReason::RateLimited,
                    );
                }
                return None;
            }

            info!("Block Engine Bundle Received, ID: {:?}", bundle.uuid);
            let packet_batch = PacketBatch::from(
                bundle
                    .bundle?
                    .packets
                    .into_iter()
                    .map(|proto| {
                        let mut packet = proto_packet_to_packet(proto);
                        BlockEngineStage::apply_bundle_source_ipv4_fallback(
                            &mut packet,
                            block_engine_ipv4,
                        );
                        packet
                    })
                    .collect::<Vec<BytesPacket>>(),
            );
            bundle_packets += packet_batch.len() as u64;
            Some(PacketBundle::new(
                packet_batch,
                bundle.uuid,
                block_engine_uuid.to_string(),
            ))
        })
        .collect();
    block_engine_stats
        .num_bundles
        .add_assign(bundles.len() as u64);
    block_engine_stats
        .num_bundle_packets
        .add_assign(bundle_packets);
    bundles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(block_engine_url: &str) -> BlockEngineConfig {
        BlockEngineConfig {
            block_engine_url: block_engine_url.to_string(),
            ..BlockEngineConfig::default()
        }
    }

    fn entry(url: &str, uuid: &str) -> BlockEngineEntry {
        BlockEngineEntry {
            url: url.to_string(),
            uuid: uuid.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn block_engine_config_validation_uses_runtime_endpoint_builder() {
        assert!(BlockEngineStage::is_valid_block_engine_config(&config(
            "https://localhost:443"
        )));
        assert!(!BlockEngineStage::is_valid_block_engine_config(&config(
            "not a valid url"
        )));
    }

    #[test]
    fn test_merged_secondary_block_engine_entries_applies_blocklist() {
        let admin_entries = vec![
            entry("https://admin-1", "admin-1"),
            entry("https://blocked", "blocked"),
        ];
        let onchain_entries = vec![
            entry("https://onchain-1", "onchain-1"),
            entry("https://blocked-onchain", "blocked"),
        ];
        let blocklist = vec!["blocked".to_string()];

        let merged = merged_secondary_block_engine_entries(
            &admin_entries,
            Some(onchain_entries),
            &blocklist,
        );

        assert_eq!(
            merged,
            vec![
                entry("https://admin-1", "admin-1"),
                entry("https://onchain-1", "onchain-1"),
            ]
        );
    }

    #[test]
    fn test_collect_block_engine_url_status() {
        let config = BlockEngineConfig {
            block_engine_url: "https://primary".to_string(),
            ..BlockEngineConfig::default()
        };
        let status = collect_block_engine_url_status(
            &config,
            &[entry("https://admin", "admin")],
            Some(vec![entry("https://onchain", "onchain")]),
            &["admin".to_string()],
        );

        assert_eq!(status.primary_url, "https://primary");
        assert_eq!(
            status.admin_secondary_entries,
            vec![entry("https://admin", "admin")]
        );
        assert_eq!(
            status.onchain_secondary_entries,
            vec![entry("https://onchain", "onchain")]
        );
        assert_eq!(status.blocklisted_uuids, vec!["admin".to_string()]);
        assert_eq!(
            status.active_secondary_entries,
            vec![entry("https://onchain", "onchain")]
        );
    }

    #[test]
    fn test_merged_secondary_keeps_multiple_onchain_urls_for_one_uuid() {
        let admin_entries = vec![entry("https://admin-1", "admin-1")];
        let onchain_entries = vec![
            BlockEngineEntry {
                url: "https://onchain-a".to_string(),
                uuid: "engine-a".to_string(),
                bundle_rate_limit: BlockEngineBundleRateLimit {
                    max_bundles: 10,
                    period_ms: 1000,
                    max_bundle_burst: 10,
                },
            },
            BlockEngineEntry {
                url: "https://onchain-b".to_string(),
                uuid: "engine-a".to_string(),
                bundle_rate_limit: BlockEngineBundleRateLimit {
                    max_bundles: 5,
                    period_ms: 1000,
                    max_bundle_burst: 5,
                },
            },
            entry("https://onchain-admin-1", "admin-1"),
        ];

        let merged =
            merged_secondary_block_engine_entries(&admin_entries, Some(onchain_entries), &[]);

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0], entry("https://admin-1", "admin-1"));
        assert_eq!(merged[1].url, "https://onchain-a");
        assert_eq!(merged[1].uuid, "engine-a");
        assert_eq!(merged[1].bundle_rate_limit.max_bundles, 10);
        assert_eq!(merged[2].url, "https://onchain-b");
        assert_eq!(merged[2].uuid, "engine-a");
        assert_eq!(merged[2].bundle_rate_limit.max_bundles, 5);
    }

    #[test]
    fn test_onchain_rate_change_is_treated_as_entry_change() {
        let previous = BlockEngineEntry {
            url: "https://onchain".to_string(),
            uuid: "engine-a".to_string(),
            bundle_rate_limit: BlockEngineBundleRateLimit {
                max_bundles: 10,
                period_ms: 1000,
                max_bundle_burst: 10,
            },
        };
        let updated = BlockEngineEntry {
            url: previous.url.clone(),
            uuid: previous.uuid.clone(),
            bundle_rate_limit: BlockEngineBundleRateLimit {
                max_bundles: 20,
                period_ms: 1000,
                max_bundle_burst: 10,
            },
        };
        assert_ne!(previous, updated);
    }

    #[test]
    fn test_parse_block_engine_entry() {
        assert_eq!(
            parse_block_engine_entry("http://example.com:15001,uuid-123").unwrap(),
            entry("http://example.com:15001", "uuid-123")
        );
        assert!(parse_block_engine_entry("missing-uuid").is_err());
    }

    #[test]
    fn test_resolve_block_engine_ipv4_literal() {
        assert_eq!(
            BlockEngineStage::resolve_block_engine_ipv4("https://1.2.3.4:443"),
            Some(Ipv4Addr::new(1, 2, 3, 4))
        );
        assert_eq!(
            BlockEngineStage::resolve_block_engine_ipv4("http://10.0.0.9"),
            Some(Ipv4Addr::new(10, 0, 0, 9))
        );
    }

    #[test]
    fn test_apply_bundle_source_ipv4_fallback() {
        let be_ip = Ipv4Addr::new(9, 9, 9, 9);
        let mut unspecified = BytesPacket::new(bytes::Bytes::new(), solana_packet::Meta::default());
        unspecified.meta_mut().addr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        BlockEngineStage::apply_bundle_source_ipv4_fallback(&mut unspecified, Some(be_ip));
        assert_eq!(unspecified.meta().addr, IpAddr::V4(be_ip));

        let valid = Ipv4Addr::new(8, 8, 8, 8);
        let mut kept = BytesPacket::new(bytes::Bytes::new(), solana_packet::Meta::default());
        kept.meta_mut().addr = IpAddr::V4(valid);
        BlockEngineStage::apply_bundle_source_ipv4_fallback(&mut kept, Some(be_ip));
        assert_eq!(kept.meta().addr, IpAddr::V4(valid));
    }

    fn fake_bundles_response(n: usize) -> block_engine::SubscribeBundlesResponse {
        use jito_protos::proto::bundle::{Bundle, BundleUuid};
        block_engine::SubscribeBundlesResponse {
            bundles: (0..n)
                .map(|i| BundleUuid {
                    uuid: format!("bundle-{i}"),
                    bundle: Some(Bundle {
                        header: None,
                        packets: vec![],
                    }),
                })
                .collect(),
        }
    }

    #[test]
    fn test_admit_unlimited_when_rate_is_zero() {
        let limiter = maybe_bundle_limiter(&BlockEngineBundleRateLimit::default());
        assert!(limiter.is_none());
        let mut stats = BlockEngineStageStats::default();
        let admitted = admit_bundles_with_rate_limit(
            fake_bundles_response(5),
            "uuid",
            true,
            None,
            false,
            &mut stats,
        );
        assert_eq!(admitted.len(), 5);
        assert_eq!(stats.num_bundles, 5);
        assert_eq!(stats.num_bundles_throttled, 0);
    }

    #[test]
    fn test_admit_throttles_beyond_burst() {
        let limiter = maybe_bundle_limiter(&BlockEngineBundleRateLimit {
            max_bundles: 1,
            period_ms: 1,
            max_bundle_burst: 1,
        })
        .expect("limiter");
        let mut stats = BlockEngineStageStats::default();
        let admitted = admit_bundles_with_rate_limit(
            fake_bundles_response(5),
            "uuid",
            false,
            Some(limiter.as_ref()),
            false,
            &mut stats,
        );
        assert_eq!(admitted.len(), 1);
        assert_eq!(stats.num_bundles, 1);
        assert_eq!(stats.num_bundles_throttled, 4);
    }

    #[test]
    fn test_admit_all_throttled_returns_empty() {
        let limiter = maybe_bundle_limiter(&BlockEngineBundleRateLimit {
            max_bundles: 1,
            period_ms: 1,
            max_bundle_burst: 1,
        })
        .expect("limiter");
        assert!(limiter.check().is_ok());
        let mut stats = BlockEngineStageStats::default();
        let admitted = admit_bundles_with_rate_limit(
            fake_bundles_response(3),
            "uuid",
            true,
            Some(limiter.as_ref()),
            false,
            &mut stats,
        );
        assert!(admitted.is_empty());
        assert_eq!(stats.num_bundles, 0);
        assert_eq!(stats.num_bundles_throttled, 3);
    }
}
