use {
    serde::Serialize,
    serde_json,
    solana_signature::Signature,
    std::net::Ipv4Addr,
};

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TxnWaterfallIn {
    pub pack_cranked: u64,
    pub pack_retained: u64,
    pub resolv_retained: u64,
    pub quic: u64,
    pub udp: u64,
    pub gossip: u64,
    pub block_engine: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TxnWaterfallOut {
    pub net_overrun: u64,
    pub quic_overrun: u64,
    pub quic_frag_drop: u64,
    pub quic_abandoned: u64,
    pub tpu_quic_invalid: u64,
    pub tpu_udp_invalid: u64,
    pub verify_overrun: u64,
    pub verify_parse: u64,
    pub verify_failed: u64,
    pub verify_duplicate: u64,
    pub dedup_duplicate: u64,
    pub resolv_lut_failed: u64,
    pub resolv_expired: u64,
    pub resolv_no_ledger: u64,
    pub resolv_ancient: u64,
    pub resolv_retained: u64,
    pub pack_invalid: u64,
    pub pack_already_executed: u64,
    pub pack_invalid_bundle: u64,
    pub pack_retained: u64,
    pub pack_leader_slow: u64,
    pub pack_wait_full: u64,
    pub pack_expired: u64,
    pub bank_invalid: u64,
    pub bank_nonce_already_advanced: u64,
    pub bank_nonce_advance_failed: u64,
    pub bank_nonce_wrong_blockhash: u64,
    pub block_success: u64,
    pub block_fail: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct TxnWaterfall {
    #[serde(rename = "in")]
    pub in_: TxnWaterfallIn,
    pub out: TxnWaterfallOut,
}

/// `in.pack_retained` / `in.resolv_retained` wire values captured from `out` at leader
/// slot end
#[derive(Clone, Copy, Debug, Default)]
pub struct RetainedSnapshot {
    pub in_pack_retained: u64,
    pub in_resolv_retained: u64,
}

impl RetainedSnapshot {
    pub fn capture_from(waterfall: &TxnWaterfall) -> Self {
        Self {
            in_pack_retained: waterfall.out.pack_retained,
            in_resolv_retained: waterfall.out.resolv_retained,
        }
    }
}

pub fn live_txn_waterfall_for_send(
    accumulated: &TxnWaterfall,
    retained: &RetainedSnapshot,
) -> TxnWaterfall {
    let mut waterfall = *accumulated;
    waterfall.in_.pack_retained = retained.in_pack_retained;
    waterfall.in_.resolv_retained = retained.in_resolv_retained;
    waterfall
}

#[derive(Serialize)]
struct LiveTxnWaterfallValue<'a> {
    next_leader_slot: Option<u64>,
    waterfall: &'a TxnWaterfall,
}

#[derive(Serialize)]
struct SummaryWsEnvelope<T> {
    topic: &'static str,
    key: &'static str,
    value: T,
}

pub fn format_live_txn_waterfall_message(
    next_leader_slot: Option<u64>,
    waterfall: &TxnWaterfall,
) -> Option<String> {
    serde_json::to_string(&SummaryWsEnvelope {
        topic: "summary",
        key: "live_txn_waterfall",
        value: LiveTxnWaterfallValue {
            next_leader_slot,
            waterfall,
        },
    })
    .ok()
}

#[derive(Serialize)]
struct BootProgressValue {
    phase: &'static str,
}

pub fn format_boot_progress_running_message() -> Option<String> {
    serde_json::to_string(&SummaryWsEnvelope {
        topic: "summary",
        key: "boot_progress",
        value: BootProgressValue { phase: "running" },
    })
    .ok()
}

/// TPU ingress path for a transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GuiTxnTpuSource {
    #[default]
    Quic,
    Udp,
    Gossip,
    Bundle,
    Send,
}

impl GuiTxnTpuSource {
    pub fn as_wire_str(self) -> &'static str {
        match self {
            Self::Quic => "quic",
            Self::Udp => "udp",
            Self::Gossip => "gossip",
            Self::Bundle => "bundle",
            Self::Send => "send",
        }
    }
}

/// Per-transaction metrics captured during a leader slot.
///
/// Timestamp fields are absolute nanoseconds (wire `txn_*_timestamps_nanos` arrays).
#[derive(Clone, Copy, Debug)]
pub struct GuiTxnRecord {
    pub signature: Signature,
    pub transaction_fee: u64,
    pub priority_fee: u64,
    pub tips: u64,
    pub timestamp_arrival_nanos: i64,
    pub timestamp_mb_start_nanos: i64,
    pub timestamp_mb_end_nanos: i64,
    pub timestamp_preload_end_nanos: i64,
    pub timestamp_start_nanos: i64,
    pub timestamp_load_end_nanos: i64,
    pub timestamp_end_nanos: i64,
    pub compute_units_requested: u32,
    pub compute_units_consumed: u32,
    pub bank_idx: u8,
    pub error_code: u8,
    pub from_bundle: bool,
    pub is_simple_vote: bool,
    pub landed: bool,
    pub source_tpu: GuiTxnTpuSource,
    pub source_ipv4: Ipv4Addr,
    pub microblock_idx: u32,
}

impl Default for GuiTxnRecord {
    fn default() -> Self {
        Self {
            signature: Signature::default(),
            transaction_fee: 0,
            priority_fee: 0,
            tips: 0,
            timestamp_arrival_nanos: 0,
            timestamp_mb_start_nanos: 0,
            timestamp_mb_end_nanos: 0,
            timestamp_preload_end_nanos: 0,
            timestamp_start_nanos: 0,
            timestamp_load_end_nanos: 0,
            timestamp_end_nanos: 0,
            compute_units_requested: 0,
            compute_units_consumed: 0,
            bank_idx: 0,
            error_code: 0,
            from_bundle: false,
            is_simple_vote: false,
            landed: false,
            source_tpu: GuiTxnTpuSource::default(),
            source_ipv4: Ipv4Addr::UNSPECIFIED,
            microblock_idx: 0,
        }
    }
}
