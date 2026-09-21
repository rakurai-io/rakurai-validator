use {
    crate::{
        banking_stage::{
            scheduler_messages::MaxAge,
            transaction_scheduler::{
                receive_and_buffer::{PacketHandlingError, TransactionViewReceiveAndBuffer},
                transaction_state_container::{
                    RuntimeTransactionView, StateContainer, TransactionViewStateContainer,
                },
            },
        },
        bundle_stage::BundleExecutionStats,
        packet_bundle::VerifiedPacketBundle,
        proxy::block_engine_stage::BlockEngineConfig,
    },
    ahash::HashSet,
    arc_swap::ArcSwap,
    arrayvec::ArrayVec,
    crossbeam_channel::Sender,
    log::info,
    min_max_heap::MinMaxHeap,
    smallvec::SmallVec,
    solana_address::Address,
    solana_clock::{BankId, MAX_PROCESSING_AGE, Slot},
    solana_hash::Hash,
    solana_perf::packet::bytes::Bytes,
    solana_pubkey::Pubkey,
    solana_runtime::bank::Bank,
    solana_runtime_transaction::{
        runtime_transaction::RuntimeTransaction, sanitize_config::sanitize_config,
        transaction_meta::TransactionMeta, transaction_with_meta::TransactionWithMeta,
    },
    solana_signature::Signature,
    solana_svm::transaction_error_metrics::TransactionErrorMetrics,
    solana_svm_timings::wallclock_timestamp_nanos,
    solana_transaction::TransactionError,
    std::{
        collections::{HashMap, VecDeque},
        str::FromStr,
        sync::{Arc, OnceLock, RwLock},
    },
};

#[derive(Debug, PartialEq, Eq)]
pub enum BundleStorageError {
    EmptyBatch,
    ContainerFull,
    PacketMarkedDiscard(usize),
    PacketFilterError((PacketHandlingError, usize /* packet index */)),
    BundleTooLarge,
    DuplicateTransaction,
    DuplicateNonce,
    ZeroTipAmount(String),
    AlreadyProcessed,
    BlockHashNotFound,
    TransactionCheckFailed,
}

struct BundleTransactionId {
    container_ids: SmallVec<[(usize, u64, u64); 5]>,
    sanitized_bank_id: BankId,
    sanitized_bank_slot: Slot,
    bundle_priority: u64,
    tip_amount: u64,
    block_engine_uuid: String,
    first_packet_remote_pubkey: Pubkey,
    is_primary: bool,
    bundle_id: String,
    timestamp_arrival_nanos: i64,
    /// Source IPv4 of the bundle's lead packet as a big-endian `u32` (0 if unset/non-IPv4).
    source_ipv4: u32,
}

impl Ord for BundleTransactionId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.bundle_priority.cmp(&other.bundle_priority)
    }
}

impl PartialOrd for BundleTransactionId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for BundleTransactionId {
    fn eq(&self, other: &Self) -> bool {
        self.bundle_priority == other.bundle_priority
    }
}

impl Eq for BundleTransactionId {}

const JITO_TIP_ACCOUNTS: [&str; 8] = [
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

/// Tip payment accounts used for GUI tip-balance deltas.
pub fn jito_tip_accounts() -> &'static std::collections::HashSet<Pubkey> {
    static JITO_TIP_ACCOUNTS_SET: OnceLock<std::collections::HashSet<Pubkey>> = OnceLock::new();
    JITO_TIP_ACCOUNTS_SET.get_or_init(|| {
        JITO_TIP_ACCOUNTS
            .iter()
            .filter_map(|account| Pubkey::from_str(account).ok())
            .collect()
    })
}

#[allow(dead_code)]
pub fn jito_tip_accounts_map() -> HashMap<Pubkey, f64> {
    JITO_TIP_ACCOUNTS
        .iter()
        .filter_map(|account| Pubkey::from_str(account).ok().map(|pubkey| (pubkey, 1.0)))
        .collect()
}

#[cfg(feature = "build_validator")]
mod postpackconf_ffi {
    use {solana_pubkey::Pubkey, solana_signature::Signature};

    unsafe extern "C" {
        #[allow(improper_ctypes)]
        #[allow(improper_ctypes_definitions)]
        /// `(any_match, tip_boost)` — see rakurai_scheduler entrypoint.
        pub fn has_postpackconf_match(
            signature: &Signature,
            remote_pubkey: &Pubkey,
        ) -> (bool, bool);
    }
}

#[cfg(feature = "build_validator")]
unsafe extern "C" {
    #[allow(improper_ctypes)]
    #[allow(unused)]
    fn fetch_bundle_tip(transaction: &RuntimeTransactionView, is_primary: bool) -> u64;
}

/// `(any_match, tip_boost)` when the signature was published via scheduler postpackconf updates.
///
/// - `any_match`: Mev or ReSell hash matches `remote_pubkey`.
/// - `tip_boost`: Mev/backrun hash matches (boost-eligible only).
fn has_postpackconf_match(signature: &Signature, remote_pubkey: &Pubkey) -> (bool, bool) {
    #[cfg(feature = "build_validator")]
    {
        // SAFETY: exported by rakurai_scheduler entrypoint from the same revision.
        return unsafe { postpackconf_ffi::has_postpackconf_match(signature, remote_pubkey) };
    }
    #[cfg(not(feature = "build_validator"))]
    {
        let _ = (signature, remote_pubkey);
        (false, false)
    }
}

pub struct BundleStorageEntry {
    pub container_ids: SmallVec<[(usize, u64 /*priority*/, u64 /*cost */); 5]>,
    pub transactions: SmallVec<[RuntimeTransactionView; 5]>,
    pub max_ages: SmallVec<[MaxAge; 5]>,
    sanitized_bank_id: BankId,
    sanitized_bank_slot: Slot,
    pub bundle_priority: u64,
    pub tip_amount: u64,
    pub block_engine_uuid: String,
    pub first_packet_remote_pubkey: Pubkey,
    pub is_primary: bool,
    pub bundle_id: String,
    /// Wall-clock nanos when the bundle entered `BundleStorage`.
    pub timestamp_arrival_nanos: i64,
    /// Source IPv4 of the bundle's lead packet as a big-endian `u32` (0 if unset/non-IPv4).
    pub source_ipv4: u32,
}

/// Result of attempting to strip a secondary backrun lead transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryBackrunStripResult {
    /// Not a secondary postpackconf backrun; leave the bundle unchanged.
    NoMatch,
    /// Postpackconf matched and lead did not fail; lead was stripped.
    Stripped { tip_boost: bool },
    /// Postpackconf matched but lead failed on-chain; caller should drop the bundle.
    Drop,
}

/// Bundle storage has two deques: one for unprocessed bundles and another for ones that exceeded
/// the cost model and need to get retried next slot.
pub struct BundleStorage {
    last_slot: Slot,
    transaction_capacity: usize,
    transaction_view_state_container: TransactionViewStateContainer,
    unprocessed_bundles: MinMaxHeap<BundleTransactionId>,
    unprocessed_bundles_secondary: MinMaxHeap<BundleTransactionId>,
    // Storage for bundles that exceeded the cost model for the slot they were last attempted
    // execution on
    cost_model_buffered_bundles: VecDeque<BundleTransactionId>,
    pub is_new_bundle_received: bool,
    pub is_loop_complete: bool,
    pub counter: u64,
}

impl BundleStorage {
    fn refresh_slot_boundary(&mut self, slot: Slot) {
        if slot != self.last_slot {
            // the cost_model_buffered_bundles has the oldest bundles at the front of the queue
            // we need to pop from the back of that queue and insert to the front of the unprocessed_bundles queue so by the time we reach the front,
            // the oldest bundle is at the front of the unprocessed_bundles queue
            while let Some(bundle) = self.cost_model_buffered_bundles.pop_back() {
                if bundle.is_primary {
                    self.unprocessed_bundles.push(bundle);
                } else {
                    self.unprocessed_bundles_secondary.push(bundle);
                }
            }

            self.last_slot = slot;
        }
    }

    pub fn peek_bundle_priority(&mut self, slot: Slot, is_primary: bool) -> Option<u64> {
        self.refresh_slot_boundary(slot);
        if is_primary {
            self.unprocessed_bundles
                .peek_max()
                .map(|bundle| bundle.bundle_priority)
        } else {
            self.unprocessed_bundles_secondary
                .peek_max()
                .map(|bundle| bundle.bundle_priority)
        }
    }

    const MAX_PACKETS_PER_BUNDLE: usize = 5;

    #[allow(unused)]
    pub fn with_capacity(transaction_capacity: usize) -> Self {
        Self {
            last_slot: Slot::default(),
            transaction_capacity,
            transaction_view_state_container: TransactionViewStateContainer::with_capacity(
                transaction_capacity,
                true,
            ),
            unprocessed_bundles: MinMaxHeap::with_capacity(transaction_capacity),
            unprocessed_bundles_secondary: MinMaxHeap::with_capacity(transaction_capacity),
            cost_model_buffered_bundles: VecDeque::with_capacity(transaction_capacity),
            is_new_bundle_received: false,
            is_loop_complete: false,
            counter: 0,
        }
    }

    pub fn unprocessed_bundles_len(&self, is_primary: bool) -> usize {
        if is_primary {
            self.unprocessed_bundles.len()
        } else {
            self.unprocessed_bundles_secondary.len()
        }
    }

    pub fn cost_model_buffered_bundles_len(&self) -> usize {
        self.cost_model_buffered_bundles.len()
    }

    pub fn num_packets_buffered(&self) -> usize {
        self.transaction_view_state_container.buffer_size()
    }

    /// Retries a bundle by inserting the transactions back into the transaction_view_state_container.
    /// The bundle is then pushed back to the cost_model_buffered_bundles queue.
    pub fn retry_bundle(&mut self, bundle: BundleStorageEntry) {
        for ((container_id, _, _), transaction) in bundle
            .container_ids
            .iter()
            .zip(bundle.transactions.into_iter())
        {
            self.transaction_view_state_container
                .get_mut_transaction_state(*container_id)
                .unwrap()
                .retry_transaction(transaction);
        }
        self.cost_model_buffered_bundles
            .push_back(BundleTransactionId {
                container_ids: bundle.container_ids,
                sanitized_bank_id: bundle.sanitized_bank_id,
                sanitized_bank_slot: bundle.sanitized_bank_slot,
                bundle_priority: bundle.bundle_priority,
                tip_amount: bundle.tip_amount,
                block_engine_uuid: bundle.block_engine_uuid,
                first_packet_remote_pubkey: bundle.first_packet_remote_pubkey,
                is_primary: bundle.is_primary,
                bundle_id: bundle.bundle_id,
                timestamp_arrival_nanos: bundle.timestamp_arrival_nanos,
                source_ipv4: bundle.source_ipv4,
            });
    }

    /// Destroys a bundle by removing the transactions from the transaction_view_state_container.
    /// It's important that transactions in the BundleStorageEntry are not used after this call
    /// as it will lead to panic inside the TransactionViewStateContainer.
    pub fn destroy_bundle(&mut self, bundle: BundleStorageEntry) {
        for (container_id, _, _) in bundle.container_ids.into_iter() {
            self.transaction_view_state_container
                .remove_by_id(container_id);
        }
    }

    /// Pops a bundle from the unprocessed_bundles queue and returns it as a BundleStorageEntry.
    /// Returns None if there are no bundles to pop.
    pub fn pop_bundle(
        &mut self,
        slot: Slot,
        bank_id: BankId,
        is_primary: bool,
    ) -> Option<BundleStorageEntry> {
        self.refresh_slot_boundary(slot);

        // only want to pop from the unprocessed bundles queue and wait for slot boundary to refresh from cost_model_buffered_bundles
        let bundles = if is_primary {
            &mut self.unprocessed_bundles
        } else {
            &mut self.unprocessed_bundles_secondary
        };

        while let Some(bundle) = bundles.pop_max() {
            if bundle.sanitized_bank_slot == slot && bundle.sanitized_bank_id != bank_id {
                for (container_id, _, _) in bundle.container_ids {
                    self.transaction_view_state_container
                        .remove_by_id(container_id);
                }
                continue;
            }

            let (bundle_transactions, bundle_max_ages): (
                SmallVec<[RuntimeTransactionView; 5]>,
                SmallVec<[MaxAge; 5]>,
            ) = bundle
                .container_ids
                .iter()
                .map(|(id, _, _)| {
                    self.transaction_view_state_container
                        .get_mut_transaction_state(*id)
                        .unwrap()
                        .take_transaction_for_scheduling()
                })
                .unzip();

            return Some(BundleStorageEntry {
                container_ids: bundle.container_ids,
                transactions: bundle_transactions,
                max_ages: bundle_max_ages,
                sanitized_bank_id: bundle.sanitized_bank_id,
                sanitized_bank_slot: bundle.sanitized_bank_slot,
                bundle_priority: bundle.bundle_priority,
                tip_amount: bundle.tip_amount,
                block_engine_uuid: bundle.block_engine_uuid,
                first_packet_remote_pubkey: bundle.first_packet_remote_pubkey,
                is_primary: bundle.is_primary,
                bundle_id: bundle.bundle_id,
                timestamp_arrival_nanos: bundle.timestamp_arrival_nanos,
                source_ipv4: bundle.source_ipv4,
            });
        }

        None
    }

    /// Pushes a bundle back onto the unprocessed_bundles queue, restoring its transactions
    /// in the container. This is the inverse of [`Self::pop_bundle`].
    pub fn push_bundle(&mut self, bundle: BundleStorageEntry, is_primary: bool) {
        for ((container_id, _, _), transaction) in bundle
            .container_ids
            .iter()
            .zip(bundle.transactions.into_iter())
        {
            self.transaction_view_state_container
                .get_mut_transaction_state(*container_id)
                .unwrap()
                .retry_transaction(transaction);
        }
        if is_primary {
            self.unprocessed_bundles.push(BundleTransactionId {
                container_ids: bundle.container_ids,
                sanitized_bank_id: bundle.sanitized_bank_id,
                sanitized_bank_slot: bundle.sanitized_bank_slot,
                bundle_priority: bundle.bundle_priority,
                tip_amount: bundle.tip_amount,
                block_engine_uuid: bundle.block_engine_uuid,
                first_packet_remote_pubkey: bundle.first_packet_remote_pubkey,
                is_primary: bundle.is_primary,
                bundle_id: bundle.bundle_id,
                timestamp_arrival_nanos: bundle.timestamp_arrival_nanos,
                source_ipv4: bundle.source_ipv4,
            });
        } else {
            self.unprocessed_bundles_secondary
                .push(BundleTransactionId {
                    container_ids: bundle.container_ids,
                    sanitized_bank_id: bundle.sanitized_bank_id,
                    sanitized_bank_slot: bundle.sanitized_bank_slot,
                    bundle_priority: bundle.bundle_priority,
                    tip_amount: bundle.tip_amount,
                    block_engine_uuid: bundle.block_engine_uuid,
                    first_packet_remote_pubkey: bundle.first_packet_remote_pubkey,
                    is_primary: bundle.is_primary,
                    bundle_id: bundle.bundle_id,
                    timestamp_arrival_nanos: bundle.timestamp_arrival_nanos,
                    source_ipv4: bundle.source_ipv4,
                });
        }
    }

    /// Preconf hashes are sent in proto `meta.addr` as `hash.to_string()` and stored in
    /// `packet.meta.remote_pubkey` in `proto_packet_to_packet`.

    /// For secondary block engine bundles whose lead transaction was already published via
    /// scheduler updates, strip the first transaction when it succeeded on-chain (backrun).
    ///
    /// Returns [`SecondaryBackrunStripResult::Stripped`] when the lead was removed (caller may
    /// tip-boost). Returns [`SecondaryBackrunStripResult::Drop`] when the lead failed on-chain.
    pub fn maybe_strip_secondary_backrun_lead_transaction(
        &mut self,
        bundle: &mut BundleStorageEntry,
        working_bank: &Bank,
    ) -> SecondaryBackrunStripResult {
        if bundle.is_primary || bundle.container_ids.len() < 2 {
            return SecondaryBackrunStripResult::NoMatch;
        }

        let Some(first_signature) = bundle
            .transactions
            .first()
            .and_then(|tx| tx.signatures().first().copied())
        else {
            return SecondaryBackrunStripResult::NoMatch;
        };

        let (matched, tip_boost) =
            has_postpackconf_match(&first_signature, &bundle.first_packet_remote_pubkey);
        if !matched {
            return SecondaryBackrunStripResult::NoMatch;
        }

        if working_bank
            .get_signature_status(&first_signature)
            .is_some_and(|status| status.is_err())
        {
            return SecondaryBackrunStripResult::Drop;
        }

        let (removed_id, _, _) = bundle.container_ids.remove(0);
        bundle.transactions.remove(0);
        bundle.max_ages.remove(0);
        self.transaction_view_state_container
            .remove_by_id(removed_id);

        info!(
            "stripped lead transaction {first_signature} from secondary block engine bundle \
             ({}); tip_boost={tip_boost}; remaining txns: {}",
            bundle.block_engine_uuid,
            bundle.container_ids.len()
        );

        SecondaryBackrunStripResult::Stripped { tip_boost }
    }

    /// Recomputes `bundle_priority` from remaining txs, optionally tip-boosting when the
    /// secondary backrun postpackconf hash matched.
    pub fn apply_postpackconf_tip_boost(bundle: &mut BundleStorageEntry) {
        let tip_amount = bundle.tip_amount.saturating_mul(120).saturating_div(100);
        let total_rewards = bundle
            .container_ids
            .iter()
            .map(|(_, reward, _)| *reward)
            .sum::<u64>()
            .saturating_add(tip_amount);
        let total_cus = bundle
            .container_ids
            .iter()
            .map(|(_, _, cus)| *cus)
            .sum::<u64>();
        bundle.bundle_priority = total_rewards
            .saturating_mul(1_000_000)
            .saturating_div(total_cus.saturating_add(1));
    }

    pub fn insert_bundle(
        &mut self,
        bundle: VerifiedPacketBundle,
        root_bank: &Bank,
        working_bank: &Bank,
        filter_keys: &HashSet<Pubkey>,
        nonce_packets: &Arc<RwLock<HashMap<(Address, Hash), (Signature, u64)>>>,
        nonce_packet_sender: &Sender<Signature>,
        block_engine_config: &ArcSwap<BlockEngineConfig>,
        bundle_id_to_stats: &mut HashMap<String, BundleExecutionStats>,
    ) -> Result<(), BundleStorageError> {
        let received_at = bundle.received_at();
        let block_engine_uuid = bundle.block_engine_uuid().to_string();
        let bundle_id = bundle.bundle_id.clone();
        let batch = bundle.take();

        let primary_block_engine_uuid = block_engine_config.load().block_engine_uuid.clone();
        let is_primary = block_engine_uuid == primary_block_engine_uuid;

        bundle_id_to_stats
            .entry(bundle_id.clone())
            .or_insert_with(|| {
                BundleExecutionStats::new_on_receive(
                    received_at,
                    block_engine_uuid.clone(),
                    is_primary,
                    working_bank.slot(),
                )
            });

        if let Some(stats) = bundle_id_to_stats.get_mut(&bundle_id) {
            stats.set_num_txs(batch.len() as u64);
        }

        let mark_drop = |stats_map: &mut HashMap<String, BundleExecutionStats>,
                         err: BundleStorageError|
         -> Result<(), BundleStorageError> {
            if let Some(stats) = stats_map.get_mut(&bundle_id) {
                stats.mark_dropped(crate::bundle_stage::BundleDropReason::from_storage_error(
                    &err,
                ));
            }
            Err(err)
        };

        // Packet checks
        if batch.is_empty() {
            return mark_drop(bundle_id_to_stats, BundleStorageError::EmptyBatch);
        }
        let batch_arrival_timestamp_nanos = wallclock_timestamp_nanos();
        if batch.len() > Self::MAX_PACKETS_PER_BUNDLE {
            if let Some(stats) = bundle_id_to_stats.get_mut(&bundle_id) {
                stats.set_signatures(BundleExecutionStats::signatures_joined_from_tx_bytes(
                    batch.iter().filter_map(|packet| packet.data(..)),
                ));
            }
            return mark_drop(bundle_id_to_stats, BundleStorageError::BundleTooLarge);
        }
        if let Some(idx) = batch
            .iter()
            .enumerate()
            .find_map(|(idx, packet)| packet.meta().discard().then_some(idx))
        {
            if let Some(stats) = bundle_id_to_stats.get_mut(&bundle_id) {
                stats.set_signatures(BundleExecutionStats::signatures_joined_from_tx_bytes(
                    batch.iter().filter_map(|packet| packet.data(..)),
                ));
            }
            return mark_drop(
                bundle_id_to_stats,
                BundleStorageError::PacketMarkedDiscard(idx),
            );
        }

        // Container checks
        if self
            .transaction_view_state_container
            .buffer_size()
            .saturating_add(batch.len())
            > self.transaction_capacity
        {
            if let Some(stats) = bundle_id_to_stats.get_mut(&bundle_id) {
                stats.set_signatures(BundleExecutionStats::signatures_joined_from_tx_bytes(
                    batch.iter().filter_map(|packet| packet.data(..)),
                ));
            }
            return mark_drop(bundle_id_to_stats, BundleStorageError::ContainerFull);
        }

        let mut container_ids = SmallVec::<[(usize, u64, u64); 5]>::new();
        let first_packet = batch.get(0).unwrap();
        let first_packet_remote_pubkey = first_packet.meta().remote_pubkey().unwrap_or_default();
        let source_ipv4 = match first_packet.meta().addr {
            std::net::IpAddr::V4(v4) => u32::from(v4),
            std::net::IpAddr::V6(_) => 0,
        };
        let sanitize_config = sanitize_config();
        let transaction_account_lock_limit = working_bank
            .get_transaction_account_lock_limit()
            .min(root_bank.get_transaction_account_lock_limit());

        let mut total_bundle_tip_amount = 0;
        let mut total_priority_fee_lamports = 0u64;
        let mut total_bundle_reward = 0;
        let mut nonce = None;
        let mut blockhash = None;
        let mut signatures: Vec<String> = Vec::with_capacity(batch.len());

        for (idx, packet) in batch.iter().enumerate() {
            // bundles shall contain all valid packets; checked above
            let bytes = Bytes::copy_from_slice(packet.data(..).unwrap());

            // try to insert the packet into the container
            match TransactionViewReceiveAndBuffer::try_handle_packet(
                bytes,
                root_bank,
                working_bank,
                transaction_account_lock_limit,
                &sanitize_config,
                filter_keys,
            ) {
                Ok((state, tx_reward)) => {
                    let container_id = self.transaction_view_state_container.insert_map_only(state);
                    let transaction_state = self
                        .transaction_view_state_container
                        .get_mut_transaction_state(container_id)
                        .unwrap();
                    let tip_amount;
                    #[cfg(feature = "build_validator")]
                    unsafe {
                        tip_amount = fetch_bundle_tip(transaction_state.transaction(), is_primary)
                    };
                    #[cfg(not(feature = "build_validator"))]
                    {
                        tip_amount = 0;
                    }

                    if let Some(sig) = transaction_state.transaction().signatures().first() {
                        signatures.push(sig.to_string());
                    }

                    let cus = transaction_state.cost();
                    total_bundle_tip_amount += tip_amount;
                    total_priority_fee_lamports += tx_reward;

                    total_bundle_reward += tip_amount + tx_reward;
                    if nonce.is_none() {
                        nonce = transaction_state
                            .transaction()
                            .as_sanitized_transaction()
                            .get_durable_nonce()
                            .copied();
                        if let Some(_nonce) = nonce {
                            blockhash =
                                Some(transaction_state.transaction().recent_blockhash().clone());
                        }
                    }

                    container_ids.push((container_id, tx_reward as u64, cus as u64));
                }
                Err(e) => {
                    // any error shall rollback any transactions added to the container
                    for (container_id, _, _) in container_ids.iter() {
                        self.transaction_view_state_container
                            .remove_by_id(*container_id);
                    }
                    // Append best-effort wire sigs for the failed packet and any not yet inserted.
                    for packet in batch.iter().skip(idx) {
                        if let Some(data) = packet.data(..) {
                            if let Some(sig) =
                                BundleExecutionStats::first_signature_from_tx_bytes(data)
                            {
                                signatures.push(sig);
                            }
                        }
                    }
                    if let Some(stats) = bundle_id_to_stats.get_mut(&bundle_id) {
                        stats.set_signatures(signatures.join(","));
                    }
                    return mark_drop(
                        bundle_id_to_stats,
                        BundleStorageError::PacketFilterError((e, idx)),
                    );
                }
            }
        }

        let signatures_joined = signatures.join(",");
        if let Some(stats) = bundle_id_to_stats.get_mut(&bundle_id) {
            stats.set_signatures(signatures_joined.clone());
        }

        // Skip age/already-processed/fee-payer checks for postpackconf backrun leads;
        // those are validated after strip at consume time (or intentionally kept if lead failed).
        let has_postpackconf = container_ids
            .first()
            .and_then(|(id, _, _)| {
                self.transaction_view_state_container
                    .get_transaction(*id)
                    .and_then(|tx| tx.signatures().first().copied())
            })
            .is_some_and(|signature| {
                has_postpackconf_match(&signature, &first_packet_remote_pubkey).0
            });
        if let Some(stats) = bundle_id_to_stats.get_mut(&bundle_id) {
            stats.set_has_postpack_confirmation(
                has_postpackconf,
                if has_postpackconf {
                    first_packet_remote_pubkey.to_string()
                } else {
                    String::new()
                },
            );
        }
        if !has_postpackconf {
            if let Err(e) = self.check_bundle_transactions(&container_ids, working_bank) {
                return mark_drop(bundle_id_to_stats, e);
            }
        }

        let is_duplicate_hashes = self.does_contain_duplicate_hashes(
            &container_ids
                .iter()
                .map(|(id, _, _)| *id)
                .collect::<Vec<usize>>(),
        );
        if is_duplicate_hashes {
            for (container_id, _, _) in container_ids.iter() {
                self.transaction_view_state_container
                    .remove_by_id(*container_id);
            }
            return mark_drop(bundle_id_to_stats, BundleStorageError::DuplicateTransaction);
        }

        #[cfg(feature = "build_validator")]
        {
            if total_bundle_tip_amount == 0 {
                for (container_id, _, _) in container_ids.iter() {
                    self.transaction_view_state_container
                        .remove_by_id(*container_id);
                }
                return mark_drop(
                    bundle_id_to_stats,
                    BundleStorageError::ZeroTipAmount(block_engine_uuid.clone()),
                );
            }
        }

        let mut total_rewards = 0;
        let mut total_cus = 0;

        for (_, reward, cus) in container_ids.iter() {
            total_rewards += reward;
            total_cus += cus;
        }

        total_rewards = total_rewards + total_bundle_tip_amount;

        let bundle_priority = total_rewards
            .saturating_mul(1_000_000)
            .saturating_div(total_cus.saturating_add(1));

        if let (Some(nonce), Some(blockhash)) = (nonce, blockhash) {
            if let Ok(nonce_packets) = nonce_packets.read() {
                if let Some((max_sig, max_reward)) = nonce_packets.get(&(nonce, blockhash)) {
                    if *max_reward >= total_bundle_reward {
                        for (container_id, _, _) in container_ids.iter() {
                            self.transaction_view_state_container
                                .remove_by_id(*container_id);
                        }
                        let _ = nonce_packet_sender.send(*max_sig);
                        return mark_drop(bundle_id_to_stats, BundleStorageError::DuplicateNonce);
                    }
                }
            }
        }

        if let Some(stats) = bundle_id_to_stats.get_mut(&bundle_id) {
            stats.fill_buffered_metrics(
                signatures_joined,
                container_ids.len() as u64,
                total_bundle_tip_amount,
                total_priority_fee_lamports,
                bundle_priority,
                total_cus,
                has_postpackconf,
            );
        }

        if is_primary {
            self.unprocessed_bundles.push(BundleTransactionId {
                container_ids,
                sanitized_bank_id: working_bank.bank_id(),
                sanitized_bank_slot: working_bank.slot(),
                bundle_priority,
                tip_amount: total_bundle_tip_amount,
                block_engine_uuid,
                first_packet_remote_pubkey,
                is_primary,
                bundle_id,
                timestamp_arrival_nanos: batch_arrival_timestamp_nanos,
                source_ipv4,
            });
        } else {
            self.unprocessed_bundles_secondary
                .push(BundleTransactionId {
                    container_ids,
                    sanitized_bank_id: working_bank.bank_id(),
                    sanitized_bank_slot: working_bank.slot(),
                    bundle_priority,
                    tip_amount: total_bundle_tip_amount,
                    block_engine_uuid,
                    first_packet_remote_pubkey,
                    is_primary,
                    bundle_id,
                    timestamp_arrival_nanos: batch_arrival_timestamp_nanos,
                    source_ipv4,
                });
        }

        Ok(())
    }

    /// Checks age, already-processed status, and fee-payer unlock for every
    /// transaction in the bundle. On any failure, removes all container ids and
    /// returns the first mapped error.
    fn check_bundle_transactions(
        &mut self,
        container_ids: &[(usize, u64, u64)],
        working_bank: &Bank,
    ) -> Result<(), BundleStorageError> {
        let check_results = {
            let mut transactions = ArrayVec::<_, { Self::MAX_PACKETS_PER_BUNDLE }>::new();
            for &(id, _, _) in container_ids {
                transactions.push(
                    self.transaction_view_state_container
                        .get_transaction(id)
                        .expect("transaction must exist"),
                );
            }
            Self::run_bundle_transaction_checks(&transactions, working_bank)
        };

        if let Err(error) = check_results {
            for (container_id, _, _) in container_ids {
                self.transaction_view_state_container
                    .remove_by_id(*container_id);
            }
            return Err(error);
        }
        Ok(())
    }

    /// Same checks as [`Self::check_bundle_transactions`], but for a popped
    /// [`BundleStorageEntry`] whose transactions have already been taken from the container.
    /// Does not remove container ids; caller should [`Self::destroy_bundle`] on failure.
    pub fn check_bundle_entry_transactions(
        bundle: &BundleStorageEntry,
        working_bank: &Bank,
    ) -> Result<(), BundleStorageError> {
        let mut transactions = ArrayVec::<_, { Self::MAX_PACKETS_PER_BUNDLE }>::new();
        for tx in bundle.transactions.iter() {
            transactions.push(tx);
        }
        Self::run_bundle_transaction_checks(&transactions, working_bank)
    }

    fn run_bundle_transaction_checks(
        transactions: &[&RuntimeTransactionView],
        working_bank: &Bank,
    ) -> Result<(), BundleStorageError> {
        let lock_results: [_; Self::MAX_PACKETS_PER_BUNDLE] = core::array::from_fn(|_| Ok(()));
        let mut error_counters = TransactionErrorMetrics::default();

        let check_results = working_bank.check_transactions::<RuntimeTransaction<_>>(
            transactions,
            &lock_results[..transactions.len()],
            MAX_PROCESSING_AGE,
            true,
            &mut error_counters,
        );

        let mut error = None;
        for result in check_results.iter() {
            match result {
                Err(TransactionError::BlockhashNotFound) => {
                    error.get_or_insert(BundleStorageError::BlockHashNotFound);
                }
                Err(TransactionError::AlreadyProcessed) => {
                    error.get_or_insert(BundleStorageError::AlreadyProcessed);
                }
                Err(_) => {
                    error.get_or_insert(BundleStorageError::TransactionCheckFailed);
                }
                Ok(_) => {}
            }
        }

        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn does_contain_duplicate_hashes(&self, container_ids: &[usize]) -> bool {
        let mut transaction_hashes = ArrayVec::<_, { Self::MAX_PACKETS_PER_BUNDLE }>::new();
        for container_id in container_ids.iter() {
            let transaction_hash = self
                .transaction_view_state_container
                .get_transaction(*container_id)
                .unwrap()
                .message_hash();
            if transaction_hashes.contains(&transaction_hash) {
                return true;
            }
            transaction_hashes.push(transaction_hash);
        }
        false
    }

    pub fn clear(&mut self) {
        while let Some(bundle) = self.unprocessed_bundles.pop_max() {
            for (id, _, _) in bundle.container_ids.iter() {
                self.transaction_view_state_container.remove_by_id(*id);
            }
        }
        while let Some(bundle) = self.unprocessed_bundles_secondary.pop_max() {
            for (id, _, _) in bundle.container_ids.iter() {
                self.transaction_view_state_container.remove_by_id(*id);
            }
        }
        for bundle in self.cost_model_buffered_bundles.drain(..) {
            for (id, _, _) in bundle.container_ids.iter() {
                self.transaction_view_state_container.remove_by_id(*id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        crate::{
            banking_stage::transaction_scheduler::{
                receive_and_buffer::PacketHandlingError,
                transaction_state_container::StateContainer,
            },
            bundle_stage::bundle_storage::{BundleStorage, BundleStorageError},
            packet_bundle::VerifiedPacketBundle,
        },
        ahash::{HashSet, HashSetExt},
        solana_account::AccountSharedData,
        solana_address_lookup_table_interface::{
            self as address_lookup_table,
            state::{AddressLookupTable, LookupTableMeta},
        },
        solana_genesis_config::GenesisConfig,
        solana_hash::Hash,
        solana_keypair::Keypair,
        solana_leader_schedule::SlotLeader,
        solana_message::{AddressLoader, AddressLookupTableAccount, VersionedMessage, v0},
        solana_perf::packet::{BytesPacket, PacketBatch},
        solana_pubkey::Pubkey,
        solana_runtime::bank::{Bank, NewBankOptions},
        solana_signer::Signer,
        solana_system_interface::instruction as system_instruction,
        solana_transaction::{Transaction, versioned::VersionedTransaction},
        std::borrow::Cow,
    };

    pub fn test_tx() -> Transaction {
        let keypair1 = Keypair::new();
        let pubkey1 = keypair1.pubkey();
        solana_system_transaction::transfer(&keypair1, &pubkey1, 42, Hash::default())
    }

    #[test]
    fn test_bundle_alt_resolution_uses_root_bank() {
        let (root_bank, _bank_forks) =
            Bank::new_with_bank_forks_for_tests(&GenesisConfig::default());
        let working_bank = Bank::new_from_parent(
            root_bank.clone(),
            SlotLeader::new_unique(),
            root_bank.slot() + 1,
        );
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let address_lookup_table_key = Pubkey::new_unique();
        let address_lookup_table = AddressLookupTable {
            meta: LookupTableMeta::default(),
            addresses: Cow::Borrowed(&[recipient]),
        };
        let data = address_lookup_table.serialize_for_tests().unwrap();
        let mut account =
            AccountSharedData::new(1, data.len(), &address_lookup_table::program::id());
        account.set_data_from_slice(&data);
        working_bank.store_account(&address_lookup_table_key, &account);

        let message = v0::Message::try_compile(
            &payer.pubkey(),
            &[system_instruction::transfer(&payer.pubkey(), &recipient, 1)],
            &[AddressLookupTableAccount {
                key: address_lookup_table_key,
                addresses: vec![recipient],
            }],
            working_bank.last_blockhash(),
        )
        .unwrap();

        assert!(
            AddressLoader::load_addresses(&working_bank, &message.address_table_lookups).is_ok()
        );

        let transaction =
            VersionedTransaction::try_new(VersionedMessage::V0(message), &[&payer]).unwrap();
        let packet = BytesPacket::from_data(transaction).unwrap();
        let bundle = VerifiedPacketBundle::new(PacketBatch::from(vec![packet]));
        let mut bundle_storage = BundleStorage::with_capacity(1);

        assert_eq!(
            bundle_storage.insert_bundle(
                bundle,
                root_bank.as_ref(),
                &working_bank,
                &HashSet::new(),
            ),
            Err(BundleStorageError::PacketFilterError((
                PacketHandlingError::ALTResolution,
                0,
            )))
        );
    }

    #[test]
    fn test_bundle_vote_only_check_uses_working_bank() {
        let (root_bank, _bank_forks) =
            Bank::new_with_bank_forks_for_tests(&GenesisConfig::default());
        let working_bank = Bank::new_from_parent_with_options(
            root_bank.clone(),
            SlotLeader::new_unique(),
            root_bank.slot() + 1,
            NewBankOptions {
                vote_only_bank: true,
            },
        );
        let packet = BytesPacket::from_data(test_tx()).unwrap();
        let bundle = VerifiedPacketBundle::new(PacketBatch::from(vec![packet]));
        let mut bundle_storage = BundleStorage::with_capacity(1);

        assert_eq!(
            bundle_storage.insert_bundle(
                bundle,
                root_bank.as_ref(),
                &working_bank,
                &HashSet::new(),
            ),
            Err(BundleStorageError::PacketFilterError((
                PacketHandlingError::Sanitization,
                0,
            )))
        );
    }

    #[test]
    fn test_bundle_too_large() {
        let mut bundle_storage = BundleStorage::with_capacity(10);

        let bank = Bank::new_for_tests(&GenesisConfig::default());
        let packets: Vec<BytesPacket> = (0..BundleStorage::MAX_PACKETS_PER_BUNDLE + 1)
            .map(|_| BytesPacket::from_data(test_tx()).unwrap())
            .collect();
        let bundle = VerifiedPacketBundle::new(PacketBatch::from(packets));
        let result = bundle_storage.insert_bundle(bundle, &bank, &bank, &HashSet::new());

        assert_matches!(result, Err(BundleStorageError::BundleTooLarge));
        assert_eq!(bundle_storage.unprocessed_bundles.len(), 0);
        assert_eq!(bundle_storage.cost_model_buffered_bundles.len(), 0);
        assert!(bundle_storage.transaction_view_state_container.is_empty());
    }

    #[test]
    fn test_bundle_marked_discard() {
        let mut bundle_storage = BundleStorage::with_capacity(10);
        let bank = Bank::new_for_tests(&GenesisConfig::default());
        let packet_1 = BytesPacket::from_data(test_tx()).unwrap();
        let mut packet_2 = BytesPacket::from_data(test_tx()).unwrap();
        packet_2.meta_mut().set_discard(true);
        let bundle = VerifiedPacketBundle::new(PacketBatch::from(vec![packet_1, packet_2]));
        let result = bundle_storage.insert_bundle(bundle, &bank, &bank, &HashSet::new());
        assert_matches!(result, Err(BundleStorageError::PacketMarkedDiscard(1)));
    }

    #[test]
    fn test_bundle_storage_exceeds_capacity() {
        let mut bundle_storage = BundleStorage::with_capacity(10);
        let bank = Bank::new_for_tests(&GenesisConfig::default());

        for i in 0..10 {
            let packet = BytesPacket::from_data(test_tx()).unwrap();
            let bundle = VerifiedPacketBundle::new(PacketBatch::from(vec![packet]));
            bundle_storage
                .insert_bundle(bundle, &bank, &bank, &HashSet::new())
                .unwrap();
            assert_eq!(bundle_storage.unprocessed_bundles.len(), i + 1);
            assert_eq!(
                bundle_storage
                    .transaction_view_state_container
                    .buffer_size(),
                i + 1
            );
        }

        let packet = BytesPacket::from_data(test_tx()).unwrap();

        let bundle = VerifiedPacketBundle::new(PacketBatch::from(vec![packet]));
        let result = bundle_storage.insert_bundle(bundle, &bank, &bank, &HashSet::new());
        assert_eq!(result, Err(BundleStorageError::ContainerFull));
        assert_eq!(bundle_storage.unprocessed_bundles.len(), 10);
        assert_eq!(
            bundle_storage
                .transaction_view_state_container
                .buffer_size(),
            10
        );
    }

    #[test]
    fn test_bundle_empty() {
        let mut bundle_storage = BundleStorage::with_capacity(10);
        let bank = Bank::new_for_tests(&GenesisConfig::default());
        let bundle = VerifiedPacketBundle::new(PacketBatch::from(vec![]));
        let result = bundle_storage.insert_bundle(bundle, &bank, &bank, &HashSet::new());
        assert_matches!(result, Err(BundleStorageError::EmptyBatch));
    }

    #[test]
    fn test_bundle_duplicate_hashes() {
        let mut bundle_storage = BundleStorage::with_capacity(10);
        let bank = Bank::new_for_tests(&GenesisConfig::default());
        let packet_1 = BytesPacket::from_data(test_tx()).unwrap();
        let packet_2 = packet_1.clone();
        let bundle = VerifiedPacketBundle::new(PacketBatch::from(vec![packet_1, packet_2]));
        let result = bundle_storage.insert_bundle(bundle, &bank, &bank, &HashSet::new());
        assert_matches!(result, Err(BundleStorageError::DuplicateTransaction));
        assert!(
            bundle_storage
                .transaction_view_state_container
                .buffer_size()
                == 0
        );
        assert!(bundle_storage.unprocessed_bundles.is_empty());
        assert!(bundle_storage.cost_model_buffered_bundles.is_empty());
    }

    #[test]
    fn test_retry_bundle() {
        let mut bundle_storage = BundleStorage::with_capacity(10);

        let bank = Bank::new_for_tests(&GenesisConfig::default());
        let bank_id = bank.bank_id();
        let packet_1 = BytesPacket::from_data(test_tx()).unwrap();
        let packet_2 = BytesPacket::from_data(test_tx()).unwrap();
        let bundle = VerifiedPacketBundle::new(PacketBatch::from(vec![packet_1, packet_2]));
        let result = bundle_storage.insert_bundle(bundle, &bank, &bank, &HashSet::new());
        assert!(result.is_ok());

        let bundle_storage_entry = bundle_storage.pop_bundle(bank.slot(), bank_id).unwrap();
        bundle_storage.retry_bundle(bundle_storage_entry);

        assert!(bundle_storage.pop_bundle(bank.slot(), bank_id).is_none());
        assert!(bundle_storage.unprocessed_bundles.is_empty());
        assert_eq!(bundle_storage.cost_model_buffered_bundles.len(), 1);
        assert_eq!(
            bundle_storage
                .transaction_view_state_container
                .buffer_size(),
            2
        );

        let bundle = bundle_storage.pop_bundle(bank.slot() + 1, bank_id).unwrap();
        bundle_storage.destroy_bundle(bundle);

        let packet = BytesPacket::from_data(test_tx()).unwrap();
        bundle_storage
            .insert_bundle(
                VerifiedPacketBundle::new(PacketBatch::from(vec![packet])),
                &bank,
                &bank,
                &HashSet::new(),
            )
            .unwrap();

        assert!(
            bundle_storage
                .pop_bundle(bank.slot(), bank.bank_id() + 1)
                .is_none()
        );
        assert!(bundle_storage.transaction_view_state_container.is_empty());
    }

    #[test]
    fn test_push_bundle() {
        let mut bundle_storage = BundleStorage::with_capacity(10);

        let bank = Bank::new_for_tests(&GenesisConfig::default());
        let packet_1 = BytesPacket::from_data(None, test_tx()).unwrap();
        let packet_2 = BytesPacket::from_data(None, test_tx()).unwrap();
        let bundle = VerifiedPacketBundle::new(PacketBatch::from(vec![packet_1, packet_2]));
        let result = bundle_storage.insert_bundle(bundle, &bank, &bank, &HashSet::new());
        assert!(result.is_ok());

        let bundle_storage_entry = bundle_storage.pop_bundle(bank.slot()).unwrap();
        bundle_storage.push_bundle(bundle_storage_entry);

        assert_eq!(bundle_storage.unprocessed_bundles.len(), 1);
        assert!(bundle_storage.cost_model_buffered_bundles.is_empty());
        assert_eq!(
            bundle_storage
                .transaction_view_state_container
                .buffer_size(),
            2
        );

        let restored = bundle_storage.pop_bundle(bank.slot()).unwrap();
        assert_eq!(restored.transactions.len(), 2);
    }

    #[test]
    fn test_bundle_blacklisted_account() {
        let mut bundle_storage = BundleStorage::with_capacity(10);
        let bank = Bank::new_for_tests(&GenesisConfig::default());
        let tx = test_tx();
        let pubkey = tx.message().account_keys[0];
        let blacklisted_accounts = HashSet::from_iter([pubkey]);
        let packet = BytesPacket::from_data(tx).unwrap();
        let bundle = VerifiedPacketBundle::new(PacketBatch::from(vec![packet]));
        let result = bundle_storage.insert_bundle(bundle, &bank, &bank, &blacklisted_accounts);
        assert_matches!(
            result,
            Err(BundleStorageError::PacketFilterError((
                PacketHandlingError::FilterKey,
                0
            )))
        );
    }

    #[test]
    fn test_retry_bundle_ordering_preserved() {
        let mut bundle_storage = BundleStorage::with_capacity(100);
        let bank = Bank::new_for_tests(&GenesisConfig::default());
        let bank_id = bank.bank_id();

        let tx_1 = test_tx();
        let tx_2 = test_tx();
        let tx_3 = test_tx();
        let tx_4 = test_tx();

        let packet_batch_1 = VerifiedPacketBundle::new(PacketBatch::from(vec![
            BytesPacket::from_data(&tx_1).unwrap(),
        ]));
        let packet_batch_2 = VerifiedPacketBundle::new(PacketBatch::from(vec![
            BytesPacket::from_data(&tx_2).unwrap(),
        ]));
        let packet_batch_3 = VerifiedPacketBundle::new(PacketBatch::from(vec![
            BytesPacket::from_data(&tx_3).unwrap(),
        ]));
        let packet_batch_4 = VerifiedPacketBundle::new(PacketBatch::from(vec![
            BytesPacket::from_data(&tx_4).unwrap(),
        ]));

        bundle_storage
            .insert_bundle(packet_batch_1, &bank, &bank, &HashSet::new())
            .unwrap();
        bundle_storage
            .insert_bundle(packet_batch_2, &bank, &bank, &HashSet::new())
            .unwrap();
        bundle_storage
            .insert_bundle(packet_batch_3, &bank, &bank, &HashSet::new())
            .unwrap();
        bundle_storage
            .insert_bundle(packet_batch_4, &bank, &bank, &HashSet::new())
            .unwrap();

        let bundle_storage_entry_1 = bundle_storage.pop_bundle(bank.slot(), bank_id).unwrap();
        assert_eq!(
            bundle_storage_entry_1.transactions[0].signatures()[0],
            tx_1.signatures[0]
        );
        let bundle_storage_entry_2 = bundle_storage.pop_bundle(bank.slot(), bank_id).unwrap();
        assert_eq!(
            bundle_storage_entry_2.transactions[0].signatures()[0],
            tx_2.signatures[0]
        );

        bundle_storage.retry_bundle(bundle_storage_entry_1);
        bundle_storage.destroy_bundle(bundle_storage_entry_2);

        let bundle_storage_entry_1 = bundle_storage.pop_bundle(bank.slot() + 1, bank_id).unwrap();
        assert_eq!(
            bundle_storage_entry_1.transactions[0].signatures()[0],
            tx_1.signatures[0]
        );
        let bundle_storage_entry_3 = bundle_storage.pop_bundle(bank.slot() + 1, bank_id).unwrap();
        assert_eq!(
            bundle_storage_entry_3.transactions[0].signatures()[0],
            tx_3.signatures[0]
        );
        let bundle_storage_entry_4 = bundle_storage.pop_bundle(bank.slot() + 1, bank_id).unwrap();
        assert_eq!(
            bundle_storage_entry_4.transactions[0].signatures()[0],
            tx_4.signatures[0]
        );
    }

    #[test]
    fn test_destroy_bundle() {
        let mut bundle_storage = BundleStorage::with_capacity(100);
        let bank = Bank::new_for_tests(&GenesisConfig::default());
        let bank_id = bank.bank_id();

        let tx_1 = test_tx();
        let tx_2 = test_tx();

        let packet_batch_1 = VerifiedPacketBundle::new(PacketBatch::from(vec![
            BytesPacket::from_data(&tx_1).unwrap(),
        ]));
        let packet_batch_2 = VerifiedPacketBundle::new(PacketBatch::from(vec![
            BytesPacket::from_data(&tx_2).unwrap(),
        ]));

        bundle_storage
            .insert_bundle(packet_batch_1, &bank, &bank, &HashSet::new())
            .unwrap();
        bundle_storage
            .insert_bundle(packet_batch_2, &bank, &bank, &HashSet::new())
            .unwrap();

        let bundle_storage_entry_1 = bundle_storage.pop_bundle(bank.slot(), bank_id).unwrap();
        bundle_storage.destroy_bundle(bundle_storage_entry_1);
        assert!(
            bundle_storage
                .transaction_view_state_container
                .buffer_size()
                == 1
        );
        let bundle_storage_entry_2 = bundle_storage.pop_bundle(bank.slot(), bank_id).unwrap();
        bundle_storage.destroy_bundle(bundle_storage_entry_2);
        assert!(
            bundle_storage
                .transaction_view_state_container
                .buffer_size()
                == 0
        );
    }

    #[test]
    fn test_clear() {
        let mut bundle_storage = BundleStorage::with_capacity(100);
        let bank = Bank::new_for_tests(&GenesisConfig::default());

        let tx_1 = test_tx();
        let tx_2 = test_tx();

        let packet_batch_1 = VerifiedPacketBundle::new(PacketBatch::from(vec![
            BytesPacket::from_data(&tx_1).unwrap(),
        ]));
        let packet_batch_2 = VerifiedPacketBundle::new(PacketBatch::from(vec![
            BytesPacket::from_data(&tx_2).unwrap(),
        ]));

        bundle_storage
            .insert_bundle(packet_batch_1, &bank, &bank, &HashSet::new())
            .unwrap();
        bundle_storage
            .insert_bundle(packet_batch_2, &bank, &bank, &HashSet::new())
            .unwrap();

        bundle_storage.clear();
        assert!(bundle_storage.unprocessed_bundles.is_empty());
        assert!(bundle_storage.cost_model_buffered_bundles.is_empty());
        assert!(
            bundle_storage
                .transaction_view_state_container
                .buffer_size()
                == 0
        );
    }
}
