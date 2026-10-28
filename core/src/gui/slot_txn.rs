use {
    crate::banking_stage::committer::CommitTransactionDetails,
    crate::gui::{metrics::GuiTxnRecord, GuiTxnScheduleInfo},
    crate::gui::metrics::GuiTxnTpuSource,
    solana_clock::Slot,
    solana_runtime_transaction::transaction_with_meta::TransactionWithMeta,
    solana_signature::Signature,
    solana_svm_timings::ExecuteGuiTimestamps,
    solana_transaction_error::TransactionError,
};

/// Per-leader-slot transaction history served by `slot.query_transactions`.
#[derive(Clone, Debug)]
pub struct SlotTxnHistory {
    pub start_timestamp_nanos: i64,
    pub target_end_timestamp_nanos: i64,
    pub txns: Vec<GuiTxnRecord>,
    /// Next Pack-like microblock id for this slot (one id per GUI txn batch).
    next_microblock_idx: u32,
}

impl SlotTxnHistory {
    pub fn new(start_timestamp_nanos: i64, target_end_timestamp_nanos: i64) -> Self {
        Self {
            start_timestamp_nanos,
            target_end_timestamp_nanos,
            txns: Vec::new(),
            next_microblock_idx: 0,
        }
    }

    pub(crate) fn alloc_microblock_idx(&mut self) -> u32 {
        let id = self.next_microblock_idx;
        self.next_microblock_idx = self.next_microblock_idx.saturating_add(1);
        id
    }
}

/// Raw per-txn data from the consume worker. Record assembly happens on the GUI thread.
#[derive(Clone, Debug)]
pub struct GuiTxnBatchItem {
    pub signature: Signature,
    pub compute_units_requested: u32,
    pub bank_idx: u8,
    pub is_simple_vote: bool,
    pub timestamp_mb_start_nanos: i64,
    pub timestamp_mb_end_nanos: i64,
    pub timestamp_arrival_nanos: i64,
    pub source_ipv4: u32,
    pub source_tpu: GuiTxnTpuSource,
    pub commit_detail: Option<CommitTransactionDetails>,
}

/// Execute output + schedule metadata sent pipeline → GUI. `GuiTxnBatchItem` assembly runs on
/// the GUI thread in [`assemble_gui_txn_batch_items`].
#[derive(Clone, Debug)]
pub struct GuiTxnBatchPayload {
    pub slot: Slot,
    pub gui_timestamps_per_tx: Vec<ExecuteGuiTimestamps>,
    pub microblock_end_timestamp_nanos: i64,
    pub gui_schedule_info: Vec<GuiTxnScheduleInfo>,
    pub compute_units_requested: Vec<u32>,
    pub commit_details: Option<Vec<CommitTransactionDetails>>,
    pub signatures: Vec<Signature>,
    pub is_simple_vote: Vec<bool>,
}

/// Pipeline → GUI transaction capture. `slot` comes from the working bank at consume time.
#[derive(Clone, Debug)]
pub enum GuiTxnEvent {
    TxnBatch(GuiTxnBatchPayload),
}

/// Build GUI commit details for a batch.
///
/// On PoH/recorder failure (`Err`), emit per-txn `NotCommitted(CommitCancelled)` so the
/// frontend shows error 39 instead of green success with `landed=false`.
pub fn gui_commit_details_for_batch<E>(
    commit_transactions_result: &Result<Vec<CommitTransactionDetails>, E>,
    txn_count: usize,
) -> Option<Vec<CommitTransactionDetails>> {
    match commit_transactions_result {
        Ok(details) => Some(details.clone()),
        Err(_) if txn_count > 0 => Some(vec![
            CommitTransactionDetails::NotCommitted(TransactionError::CommitCancelled);
            txn_count
        ]),
        Err(_) => None,
    }
}

/// Minimal per-txn metadata extracted on the hot path (signature + vote flag only).
pub fn capture_gui_txn_tx_metadata<Tx: TransactionWithMeta>(
    txs: &[Tx],
) -> (Vec<Signature>, Vec<bool>) {
    let mut signatures = Vec::with_capacity(txs.len());
    let mut is_simple_vote = Vec::with_capacity(txs.len());
    for tx in txs {
        signatures.push(*tx.signature());
        is_simple_vote.push(tx.is_simple_vote_transaction());
    }
    (signatures, is_simple_vote)
}

/// Build [`GuiTxnBatchItem`] rows on the GUI thread from a [`GuiTxnBatchPayload`].
pub fn assemble_gui_txn_batch_items(payload: &GuiTxnBatchPayload) -> Vec<GuiTxnBatchItem> {
    payload
        .gui_schedule_info
        .iter()
        .enumerate()
        .map(|(index, schedule)| {
            let per_tx_end_nanos = payload
                .gui_timestamps_per_tx
                .get(index)
                .map(|timestamps| timestamps.timestamp_end_nanos)
                .filter(|&timestamp| timestamp > 0)
                .unwrap_or(payload.microblock_end_timestamp_nanos);
            GuiTxnBatchItem {
            signature: payload.signatures.get(index).copied().unwrap_or_default(),
            compute_units_requested: payload
                .compute_units_requested
                .get(index)
                .copied()
                .unwrap_or(0),
            bank_idx: schedule.bank_idx,
            is_simple_vote: payload.is_simple_vote.get(index).copied().unwrap_or(false),
            timestamp_mb_start_nanos: if schedule.microblock_start_timestamp_nanos > 0 {
                schedule.microblock_start_timestamp_nanos
            } else {
                schedule.timestamp_arrival_nanos
            },
            timestamp_mb_end_nanos: per_tx_end_nanos,
            timestamp_arrival_nanos: schedule.timestamp_arrival_nanos,
            source_ipv4: schedule.source_ipv4,
            source_tpu: schedule.source_tpu,
            commit_detail: payload
                .commit_details
                .as_ref()
                .and_then(|details| details.get(index).cloned()),
        }
        })
        .collect()
}
