use {
    crate::gui::slot_store::{SlotRankingsTotals, SlotTxnStore},
    arc_swap::ArcSwap,
    serde::Serialize,
    solana_clock::{Epoch, Slot},
    solana_keypair::Keypair,
    solana_leader_schedule::NUM_CONSECUTIVE_LEADER_SLOTS,
    solana_pubkey::Pubkey,
    solana_runtime::{bank::Bank, bank_forks::BankForks, leader_schedule_utils::leader_schedule},
    solana_signer::Signer,
    solana_tls_utils::NotifyKeyUpdate,
    std::{
        collections::HashMap,
        sync::{Arc, RwLock},
    },
    tokio::sync::broadcast,
};

/// Bank + identity context for frontend bootstrap messages.
#[derive(Clone)]
pub struct GuiContext {
    /// Live validator identity (updated on admin `setIdentity`).
    pub identity: Arc<ArcSwap<Pubkey>>,
    pub bank_forks: Arc<RwLock<BankForks>>,
    /// Number of GUI bank tiles (vote + bundle + consume workers).
    pub bank_tile_count: usize,
}

impl GuiContext {
    pub fn identity_pubkey(&self) -> Pubkey {
        **self.identity.load()
    }
}

/// Pushes identity updates to connected GUI clients on admin `setIdentity`.
pub struct GuiIdentityUpdater {
    identity: Arc<ArcSwap<Pubkey>>,
    ws_sender: broadcast::Sender<String>,
}

impl GuiIdentityUpdater {
    pub fn new(identity: Arc<ArcSwap<Pubkey>>, ws_sender: broadcast::Sender<String>) -> Self {
        Self {
            identity,
            ws_sender,
        }
    }
}

impl NotifyKeyUpdate for GuiIdentityUpdater {
    fn update_key(&self, key: &Keypair) -> Result<(), Box<dyn core::error::Error>> {
        let new_identity = key.pubkey();
        self.identity.store(Arc::new(new_identity));
        if let Some(msg) = format_identity_key_message(&new_identity) {
            let _ = self.ws_sender.send(msg);
        }
        if let Some(msg) = format_stub_self_peer_message(&new_identity) {
            let _ = self.ws_sender.send(msg);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct GuiWsSnapshotRequest {
    pub reply: tokio::sync::oneshot::Sender<Vec<String>>,
}

pub fn new_snapshot_channel() -> (
    tokio::sync::mpsc::Sender<GuiWsSnapshotRequest>,
    tokio::sync::mpsc::Receiver<GuiWsSnapshotRequest>,
) {
    tokio::sync::mpsc::channel(8)
}

#[derive(Serialize)]
struct SummaryEnvelope<T> {
    topic: &'static str,
    key: &'static str,
    value: T,
}

#[derive(Serialize)]
struct EpochEnvelope {
    topic: &'static str,
    key: &'static str,
    value: EpochWire,
}

#[derive(Serialize)]
struct EpochWire {
    epoch: Epoch,
    start_time_nanos: Option<String>,
    end_time_nanos: Option<String>,
    start_slot: Slot,
    end_slot: Slot,
    excluded_stake_lamports: u64,
    staked_pubkeys: Vec<String>,
    staked_lamports: Vec<u64>,
    leader_slots: Vec<u64>,
}

#[derive(Serialize)]
struct BootProgressWire {
    phase: &'static str,
    catching_up_first_replay_slot: Slot,
}

#[derive(Serialize)]
struct TileWire {
    kind: &'static str,
    kind_id: u8,
}

#[derive(Serialize)]
struct SlotUpdateEnvelope<'a> {
    topic: &'static str,
    key: &'static str,
    value: SlotUpdateValue<'a>,
}

#[derive(Serialize)]
struct SlotUpdateValue<'a> {
    publish: &'a crate::gui::slot_query::SlotPublishWire,
}

pub fn connect_snapshot_messages(ctx: &GuiContext) -> Vec<String> {
    let mut messages = Vec::new();
    if let Some(msg) = format_tiles_message(ctx.bank_tile_count) {
        messages.push(msg);
    }
    if let Some(msg) = format_identity_key_message(&ctx.identity_pubkey()) {
        messages.push(msg);
    }
    messages.extend(format_epoch_messages(ctx));
    if let Some(msg) = format_boot_progress_message(ctx) {
        messages.push(msg);
    }
    if let Some(msg) = format_startup_progress_running_message(ctx) {
        messages.push(msg);
    }
    if let Some(msg) = format_stub_self_peer_message(&ctx.identity_pubkey()) {
        messages.push(msg);
    }
    if let Some(msg) = format_slot_update_message(ctx) {
        messages.push(msg);
    }
    messages
}

#[derive(Serialize)]
struct SlotRankingsWire {
    slots_largest_tips: Vec<Slot>,
    vals_largest_tips: Vec<u64>,
    slots_smallest_tips: Vec<Slot>,
    vals_smallest_tips: Vec<u64>,
    slots_largest_fees: Vec<Slot>,
    vals_largest_fees: Vec<u64>,
    slots_smallest_fees: Vec<Slot>,
    vals_smallest_fees: Vec<u64>,
    slots_largest_rewards: Vec<Slot>,
    vals_largest_rewards: Vec<u64>,
    slots_smallest_rewards: Vec<Slot>,
    vals_smallest_rewards: Vec<u64>,
    slots_largest_duration: Vec<Slot>,
    vals_largest_duration: Vec<u64>,
    slots_smallest_duration: Vec<Slot>,
    vals_smallest_duration: Vec<u64>,
    slots_largest_compute_units: Vec<Slot>,
    vals_largest_compute_units: Vec<u64>,
    slots_smallest_compute_units: Vec<Slot>,
    vals_smallest_compute_units: Vec<u64>,
    slots_largest_skipped: Vec<Slot>,
    vals_largest_skipped: Vec<u64>,
    slots_smallest_skipped: Vec<Slot>,
    vals_smallest_skipped: Vec<u64>,
}

#[derive(Serialize)]
struct SlotRankingsEnvelope {
    topic: &'static str,
    key: &'static str,
    id: u64,
    value: SlotRankingsWire,
}

pub fn format_query_rankings_response(id: u64, rankings: &SlotRankingsTotals) -> Option<String> {
    let mut wire = empty_slot_rankings_wire();
    wire.slots_largest_fees = rankings.largest_fees.iter().map(|(slot, _)| *slot).collect();
    wire.vals_largest_fees = rankings.largest_fees.iter().map(|(_, value)| *value).collect();
    wire.slots_largest_tips = rankings.largest_tips.iter().map(|(slot, _)| *slot).collect();
    wire.vals_largest_tips = rankings.largest_tips.iter().map(|(_, value)| *value).collect();
    wire.slots_largest_rewards = rankings
        .largest_rewards
        .iter()
        .map(|(slot, _)| *slot)
        .collect();
    wire.vals_largest_rewards = rankings
        .largest_rewards
        .iter()
        .map(|(_, value)| *value)
        .collect();
    serde_json::to_string(&SlotRankingsEnvelope {
        topic: "slot",
        key: "query_rankings",
        id,
        value: wire,
    })
    .ok()
}

fn empty_slot_rankings_wire() -> SlotRankingsWire {
    SlotRankingsWire {
        slots_largest_tips: Vec::new(),
        vals_largest_tips: Vec::new(),
        slots_smallest_tips: Vec::new(),
        vals_smallest_tips: Vec::new(),
        slots_largest_fees: Vec::new(),
        vals_largest_fees: Vec::new(),
        slots_smallest_fees: Vec::new(),
        vals_smallest_fees: Vec::new(),
        slots_largest_rewards: Vec::new(),
        vals_largest_rewards: Vec::new(),
        slots_smallest_rewards: Vec::new(),
        vals_smallest_rewards: Vec::new(),
        slots_largest_duration: Vec::new(),
        vals_largest_duration: Vec::new(),
        slots_smallest_duration: Vec::new(),
        vals_smallest_duration: Vec::new(),
        slots_largest_compute_units: Vec::new(),
        vals_largest_compute_units: Vec::new(),
        slots_smallest_compute_units: Vec::new(),
        vals_smallest_compute_units: Vec::new(),
        slots_largest_skipped: Vec::new(),
        vals_largest_skipped: Vec::new(),
        slots_smallest_skipped: Vec::new(),
        vals_smallest_skipped: Vec::new(),
    }
}

pub fn format_query_rankings_response_from_store(
    store: &SlotTxnStore,
    id: u64,
) -> Option<String> {
    format_query_rankings_response(id, &store.compute_rankings())
}

pub fn format_tiles_message(bank_tile_count: usize) -> Option<String> {
    let tiles: Vec<TileWire> = (0..bank_tile_count)
        .map(|kind_id| TileWire {
            kind: "bank",
            kind_id: kind_id as u8,
        })
        .collect();
    serde_json::to_string(&SummaryEnvelope {
        topic: "summary",
        key: "tiles",
        value: tiles,
    })
    .ok()
}

pub fn format_identity_key_message(identity: &Pubkey) -> Option<String> {
    serde_json::to_string(&SummaryEnvelope {
        topic: "summary",
        key: "identity_key",
        value: identity.to_string(),
    })
    .ok()
}

pub fn format_boot_progress_message(ctx: &GuiContext) -> Option<String> {
    let first_replay_slot = read_working_bank(ctx).map_or(0, |bank| {
        bank.epoch_schedule().get_first_slot_in_epoch(bank.epoch())
    });
    serde_json::to_string(&SummaryEnvelope {
        topic: "summary",
        key: "boot_progress",
        value: BootProgressWire {
            phase: "running",
            catching_up_first_replay_slot: first_replay_slot,
        },
    })
    .ok()
}

/// Startup overlay reads `startup_progress`.
/// `ledger_max_slot` drives `firstProcessedSlot` (`ledger_max_slot + 1`) on the Slot Details page.
pub fn format_startup_progress_running_message(ctx: &GuiContext) -> Option<String> {
    serde_json::to_string(&SummaryEnvelope {
        topic: "summary",
        key: "startup_progress",
        value: StartupProgressWire::running(ctx),
    })
    .ok()
}

/// Dismisses the startup overlay when `phase === "running"` and peers exist.
pub fn format_stub_self_peer_message(identity: &Pubkey) -> Option<String> {
    serde_json::to_string(&PeersEnvelope {
        topic: "peers",
        key: "update",
        value: PeersUpdateWire {
            add: Some(vec![PeerUpdateWire {
                identity_pubkey: identity.to_string(),
                gossip: PeerGossipWire {
                    wallclock: 0,
                    shred_version: 0,
                    version: None,
                    feature_set: None,
                    sockets: HashMap::new(),
                },
                vote: Vec::new(),
                info: None,
            }]),
            update: None,
            remove: None,
        },
    })
    .ok()
}

#[derive(Serialize)]
struct StartupProgressWire {
    phase: &'static str,
    downloading_full_snapshot_slot: Option<u64>,
    downloading_full_snapshot_peer: Option<String>,
    downloading_full_snapshot_elapsed_secs: Option<u64>,
    downloading_full_snapshot_remaining_secs: Option<u64>,
    downloading_full_snapshot_throughput: Option<u64>,
    downloading_full_snapshot_total_bytes: Option<u64>,
    downloading_full_snapshot_current_bytes: Option<u64>,
    downloading_incremental_snapshot_slot: Option<u64>,
    downloading_incremental_snapshot_peer: Option<String>,
    downloading_incremental_snapshot_elapsed_secs: Option<u64>,
    downloading_incremental_snapshot_remaining_secs: Option<u64>,
    downloading_incremental_snapshot_throughput: Option<u64>,
    downloading_incremental_snapshot_total_bytes: Option<u64>,
    downloading_incremental_snapshot_current_bytes: Option<u64>,
    ledger_slot: Option<u64>,
    ledger_max_slot: Option<u64>,
    waiting_for_supermajority_slot: Option<u64>,
    waiting_for_supermajority_stake_percent: Option<u64>,
}

impl StartupProgressWire {
    fn running(ctx: &GuiContext) -> Self {
        let (ledger_slot, ledger_max_slot) = startup_ledger_bounds(ctx);
        Self {
            phase: "running",
            downloading_full_snapshot_slot: None,
            downloading_full_snapshot_peer: None,
            downloading_full_snapshot_elapsed_secs: None,
            downloading_full_snapshot_remaining_secs: None,
            downloading_full_snapshot_throughput: None,
            downloading_full_snapshot_total_bytes: None,
            downloading_full_snapshot_current_bytes: None,
            downloading_incremental_snapshot_slot: None,
            downloading_incremental_snapshot_peer: None,
            downloading_incremental_snapshot_elapsed_secs: None,
            downloading_incremental_snapshot_remaining_secs: None,
            downloading_incremental_snapshot_throughput: None,
            downloading_incremental_snapshot_total_bytes: None,
            downloading_incremental_snapshot_current_bytes: None,
            ledger_slot,
            ledger_max_slot,
            waiting_for_supermajority_slot: None,
            waiting_for_supermajority_stake_percent: None,
        }
    }
}

/// Uses `firstProcessedSlot = ledger_max_slot + 1`.
fn startup_ledger_bounds(ctx: &GuiContext) -> (Option<u64>, Option<u64>) {
    let Some(bank) = read_working_bank(ctx) else {
        return (None, None);
    };
    let ledger_slot = bank.slot();
    if ledger_slot == 0 {
        return (None, None);
    }
    let first_slot_in_epoch = bank.epoch_schedule().get_first_slot_in_epoch(bank.epoch());
    let ledger_max_slot = first_slot_in_epoch.saturating_sub(1);
    (Some(ledger_slot), Some(ledger_max_slot))
}

#[derive(Serialize)]
struct PeersEnvelope<T> {
    topic: &'static str,
    key: &'static str,
    value: T,
}

#[derive(Serialize)]
struct PeersUpdateWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    add: Option<Vec<PeerUpdateWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    update: Option<Vec<PeerUpdateWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remove: Option<Vec<PeerRemoveWire>>,
}

#[derive(Serialize)]
struct PeerUpdateWire {
    identity_pubkey: String,
    gossip: PeerGossipWire,
    vote: Vec<serde_json::Value>,
    info: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct PeerGossipWire {
    wallclock: u64,
    shred_version: u16,
    version: Option<String>,
    feature_set: Option<u64>,
    sockets: HashMap<String, String>,
}

#[derive(Serialize)]
struct PeerRemoveWire {
    identity_pubkey: String,
}

/// Current + next epoch `epoch/new` messages
pub fn format_epoch_messages(ctx: &GuiContext) -> Vec<String> {
    let Some(bank) = read_working_bank(ctx) else {
        return Vec::new();
    };
    let current = bank.epoch();
    [current, current.saturating_add(1)]
        .into_iter()
        .filter_map(|epoch| {
            let wire = epoch_wire_from_bank(&bank, epoch)?;
            serde_json::to_string(&EpochEnvelope {
                topic: "epoch",
                key: "new",
                value: wire,
            })
            .ok()
        })
        .collect()
}

pub fn format_slot_update_message(ctx: &GuiContext) -> Option<String> {
    let bank = read_working_bank(ctx)?;
    let slot = bank.slot();
    if slot == 0 {
        return None;
    }
    let completed_slot = slot.saturating_sub(1);
    let publish = crate::gui::slot_query::publish_from_history(
        completed_slot,
        None,
        false,
    );
    serde_json::to_string(&SlotUpdateEnvelope {
        topic: "slot",
        key: "update",
        value: SlotUpdateValue { publish: &publish },
    })
    .ok()
}

fn read_working_bank(ctx: &GuiContext) -> Option<Arc<Bank>> {
    let bank_forks = ctx.bank_forks.read().ok()?;
    Some(bank_forks.working_bank())
}

fn epoch_wire_from_bank(bank: &Bank, epoch: Epoch) -> Option<EpochWire> {
    let schedule = leader_schedule(epoch, bank)?;
    let vote_accounts = bank.epoch_vote_accounts(epoch)?;

    let mut node_stake: HashMap<Pubkey, u64> = HashMap::new();
    for (_vote_pubkey, (stake, vote_account)) in vote_accounts.iter() {
        *node_stake
            .entry(*vote_account.node_pubkey())
            .or_default() += stake;
    }

    let mut staked_pubkeys: Vec<Pubkey> = node_stake.keys().copied().collect();
    staked_pubkeys.sort();
    let staked_lamports: Vec<u64> = staked_pubkeys
        .iter()
        .map(|pubkey| node_stake[pubkey])
        .collect();
    let id_to_index: HashMap<Pubkey, usize> = staked_pubkeys
        .iter()
        .enumerate()
        .map(|(idx, pubkey)| (*pubkey, idx))
        .collect();

    let slots_in_epoch = bank.get_slots_in_epoch(epoch) as usize;
    let num_groups = slots_in_epoch / NUM_CONSECUTIVE_LEADER_SLOTS.get();
    let mut leader_slots = Vec::with_capacity(num_groups);
    for group in 0..num_groups {
        let slot_index = group as u64 * NUM_CONSECUTIVE_LEADER_SLOTS.get() as u64;
        let leader_id = schedule[slot_index].id;
        let leader_idx = id_to_index.get(&leader_id).copied().unwrap_or(0) as u64;
        leader_slots.push(leader_idx);
    }

    Some(EpochWire {
        epoch,
        start_time_nanos: None,
        end_time_nanos: None,
        start_slot: bank.epoch_schedule().get_first_slot_in_epoch(epoch),
        end_slot: bank.epoch_schedule().get_last_slot_in_epoch(epoch),
        excluded_stake_lamports: 0,
        staked_pubkeys: staked_pubkeys.iter().map(ToString::to_string).collect(),
        staked_lamports,
        leader_slots,
    })
}
