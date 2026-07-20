use {
    crate::gui::{
        metrics::GuiTxnRecord,
        slot_txn::SlotTxnHistory,
    },
    serde::Serialize,
    solana_clock::Slot,
    solana_cost_model::block_cost_limits::MAX_BLOCK_UNITS,
};

#[derive(Debug)]
pub struct GuiWsQuery {
    pub slot: Slot,
    pub id: u64,
    pub reply: tokio::sync::oneshot::Sender<String>,
}

#[derive(Debug)]
pub struct GuiWsRankingsQuery {
    pub id: u64,
    pub reply: tokio::sync::oneshot::Sender<String>,
}

pub fn new_query_channel() -> (
    tokio::sync::mpsc::Sender<GuiWsQuery>,
    tokio::sync::mpsc::Receiver<GuiWsQuery>,
) {
    tokio::sync::mpsc::channel(64)
}

pub fn new_rankings_channel() -> (
    tokio::sync::mpsc::Sender<GuiWsRankingsQuery>,
    tokio::sync::mpsc::Receiver<GuiWsRankingsQuery>,
) {
    tokio::sync::mpsc::channel(16)
}

#[derive(Serialize)]
pub struct SlotPublishWire {
    pub slot: Slot,
    pub mine: bool,
    pub skipped: bool,
    pub level: &'static str,
    pub success_nonvote_transaction_cnt: Option<u64>,
    pub failed_nonvote_transaction_cnt: Option<u64>,
    pub success_vote_transaction_cnt: Option<u64>,
    pub failed_vote_transaction_cnt: Option<u64>,
    pub priority_fee: Option<u64>,
    pub transaction_fee: Option<u64>,
    pub tips: Option<u64>,
    pub max_compute_units: Option<u64>,
    pub compute_units: Option<u64>,
    pub duration_nanos: Option<u64>,
    pub completed_time_nanos: Option<u64>,
    pub vote_latency: Option<u64>,
}

#[derive(Serialize)]
pub struct SlotTransactionsWire {
    pub start_timestamp_nanos: String,
    pub target_end_timestamp_nanos: String,
    pub txn_mb_start_timestamps_nanos: Vec<String>,
    pub txn_mb_end_timestamps_nanos: Vec<String>,
    pub txn_compute_units_requested: Vec<u32>,
    pub txn_compute_units_consumed: Vec<u32>,
    pub txn_transaction_fee: Vec<u64>,
    pub txn_priority_fee: Vec<u64>,
    pub txn_tips: Vec<u64>,
    pub txn_error_code: Vec<u8>,
    pub txn_from_bundle: Vec<bool>,
    pub txn_is_simple_vote: Vec<bool>,
    pub txn_bank_idx: Vec<u8>,
    pub txn_preload_end_timestamps_nanos: Vec<String>,
    pub txn_start_timestamps_nanos: Vec<String>,
    pub txn_load_end_timestamps_nanos: Vec<String>,
    pub txn_end_timestamps_nanos: Vec<String>,
    pub txn_arrival_timestamps_nanos: Vec<String>,
    pub txn_microblock_id: Vec<u32>,
    pub txn_landed: Vec<bool>,
    pub txn_signature: Vec<String>,
    pub txn_source_ipv4: Vec<String>,
    pub txn_source_tpu: Vec<&'static str>,
}

#[derive(Serialize)]
struct SlotQueryResponseValue {
    publish: SlotPublishWire,
    transactions: Option<SlotTransactionsWire>,
}

#[derive(Serialize)]
struct SlotWsEnvelope<'a> {
    topic: &'static str,
    key: &'static str,
    id: u64,
    value: &'a SlotQueryResponseValue,
}

pub fn format_query_transactions_response(
    slot: Slot,
    id: u64,
    history: Option<&SlotTxnHistory>,
    is_active_leader: bool,
) -> Option<String> {
    let transactions = if is_active_leader {
        None
    } else {
        history.map(slot_transactions_from_history)
    };
    let publish = publish_from_history(slot, history, is_active_leader);
    let value = SlotQueryResponseValue {
        publish,
        transactions,
    };
    serde_json::to_string(&SlotWsEnvelope {
        topic: "slot",
        key: "query_transactions",
        id,
        value: &value,
    })
    .ok()
}

pub fn publish_from_history(
    slot: Slot,
    history: Option<&SlotTxnHistory>,
    is_active_leader: bool,
) -> SlotPublishWire {
    let level = if is_active_leader {
        "incomplete"
    } else {
        "completed"
    };

    let mut success_nonvote = 0u64;
    let mut failed_nonvote = 0u64;
    let mut success_vote = 0u64;
    let mut failed_vote = 0u64;
    let mut priority_fee = 0u64;
    let mut transaction_fee = 0u64;
    let mut tips = 0u64;
    let mut compute_units = 0u64;
    let mut duration_nanos = 0u64;
    let mut completed_time_nanos = 0u64;

    if let Some(history) = history {
        let (start, end) = slot_time_bounds(&history.txns);
        if history.start_timestamp_nanos > 0 {
            duration_nanos = history
                .target_end_timestamp_nanos
                .saturating_sub(history.start_timestamp_nanos) as u64;
        } else if end > start {
            duration_nanos = (end - start) as u64;
        }
        completed_time_nanos = if history.target_end_timestamp_nanos > 0 {
            history.target_end_timestamp_nanos as u64
        } else {
            end as u64
        };

        for txn in &history.txns {
            priority_fee = priority_fee.saturating_add(txn.priority_fee);
            transaction_fee = transaction_fee.saturating_add(txn.transaction_fee);
            tips = tips.saturating_add(txn.tips);
            compute_units = compute_units.saturating_add(txn.compute_units_consumed as u64);

            if txn.is_simple_vote {
                if txn.landed && txn.error_code == 0 {
                    success_vote += 1;
                } else {
                    failed_vote += 1;
                }
            } else if txn.landed && txn.error_code == 0 {
                success_nonvote += 1;
            } else {
                failed_nonvote += 1;
            }
        }
    }

    SlotPublishWire {
        slot,
        mine: true,
        skipped: false,
        level,
        success_nonvote_transaction_cnt: Some(success_nonvote),
        failed_nonvote_transaction_cnt: Some(failed_nonvote),
        success_vote_transaction_cnt: Some(success_vote),
        failed_vote_transaction_cnt: Some(failed_vote),
        priority_fee: Some(priority_fee),
        transaction_fee: Some(transaction_fee),
        tips: Some(tips),
        max_compute_units: Some(MAX_BLOCK_UNITS),
        compute_units: Some(compute_units),
        duration_nanos: Some(duration_nanos),
        completed_time_nanos: Some(completed_time_nanos),
        vote_latency: None,
    }
}

fn slot_transactions_from_history(history: &SlotTxnHistory) -> SlotTransactionsWire {
    let (start, end) = slot_time_bounds(&history.txns);
    let start_timestamp_nanos = if history.start_timestamp_nanos > 0 {
        history.start_timestamp_nanos
    } else {
        start
    };
    let target_end_timestamp_nanos = if history.target_end_timestamp_nanos > 0 {
        history.target_end_timestamp_nanos
    } else {
        end
    };

    let mut wire = SlotTransactionsWire {
        start_timestamp_nanos: nano_str(start_timestamp_nanos),
        target_end_timestamp_nanos: nano_str(target_end_timestamp_nanos),
        txn_mb_start_timestamps_nanos: Vec::with_capacity(history.txns.len()),
        txn_mb_end_timestamps_nanos: Vec::with_capacity(history.txns.len()),
        txn_compute_units_requested: Vec::with_capacity(history.txns.len()),
        txn_compute_units_consumed: Vec::with_capacity(history.txns.len()),
        txn_transaction_fee: Vec::with_capacity(history.txns.len()),
        txn_priority_fee: Vec::with_capacity(history.txns.len()),
        txn_tips: Vec::with_capacity(history.txns.len()),
        txn_error_code: Vec::with_capacity(history.txns.len()),
        txn_from_bundle: Vec::with_capacity(history.txns.len()),
        txn_is_simple_vote: Vec::with_capacity(history.txns.len()),
        txn_bank_idx: Vec::with_capacity(history.txns.len()),
        txn_preload_end_timestamps_nanos: Vec::with_capacity(history.txns.len()),
        txn_start_timestamps_nanos: Vec::with_capacity(history.txns.len()),
        txn_load_end_timestamps_nanos: Vec::with_capacity(history.txns.len()),
        txn_end_timestamps_nanos: Vec::with_capacity(history.txns.len()),
        txn_arrival_timestamps_nanos: Vec::with_capacity(history.txns.len()),
        txn_microblock_id: Vec::with_capacity(history.txns.len()),
        txn_landed: Vec::with_capacity(history.txns.len()),
        txn_signature: Vec::with_capacity(history.txns.len()),
        txn_source_ipv4: Vec::with_capacity(history.txns.len()),
        txn_source_tpu: Vec::with_capacity(history.txns.len()),
    };

    for txn in &history.txns {
        wire.txn_mb_start_timestamps_nanos
            .push(nano_str(mb_start_nanos(txn)));
        wire.txn_mb_end_timestamps_nanos
            .push(nano_str(mb_end_nanos(txn)));
        wire.txn_compute_units_requested
            .push(txn.compute_units_requested);
        wire.txn_compute_units_consumed
            .push(txn.compute_units_consumed);
        wire.txn_transaction_fee.push(txn.transaction_fee);
        wire.txn_priority_fee.push(txn.priority_fee);
        wire.txn_tips.push(txn.tips);
        wire.txn_error_code.push(txn.error_code);
        wire.txn_from_bundle.push(txn.from_bundle);
        wire.txn_is_simple_vote.push(txn.is_simple_vote);
        wire.txn_bank_idx.push(txn.bank_idx);
        wire.txn_preload_end_timestamps_nanos
            .push(nano_str(txn.timestamp_preload_end_nanos));
        wire.txn_start_timestamps_nanos
            .push(nano_str(txn.timestamp_start_nanos));
        wire.txn_load_end_timestamps_nanos
            .push(nano_str(txn.timestamp_load_end_nanos));
        wire.txn_end_timestamps_nanos
            .push(nano_str(txn.timestamp_end_nanos));
        wire.txn_arrival_timestamps_nanos
            .push(nano_str(txn.timestamp_arrival_nanos));
        wire.txn_microblock_id.push(txn.microblock_idx);
        wire.txn_landed.push(txn.landed);
        wire.txn_signature.push(txn.signature.to_string());
        wire.txn_source_ipv4
            .push(txn.source_ipv4.to_string());
        wire.txn_source_tpu.push(txn.source_tpu.as_wire_str());
    }

    wire
}

fn mb_start_nanos(txn: &GuiTxnRecord) -> i64 {
    // Per-txn arrival spreads the chart; batch microblock start is identical for the whole batch.
    // if txn.timestamp_arrival_nanos > 0 {
    //     return txn.timestamp_arrival_nanos;
    // }
    if txn.timestamp_mb_start_nanos > 0 {
        return txn.timestamp_mb_start_nanos;
    }
    txn.timestamp_preload_end_nanos
}

fn mb_end_nanos(txn: &GuiTxnRecord) -> i64 {
    if txn.timestamp_mb_end_nanos > 0 {
        txn.timestamp_mb_end_nanos
    } else {
        txn.timestamp_end_nanos
    }
}

fn slot_time_bounds(txns: &[GuiTxnRecord]) -> (i64, i64) {
    let mut start = i64::MAX;
    let mut end = 0i64;

    for txn in txns {
        for ts in [
            txn.timestamp_arrival_nanos,
            txn.timestamp_mb_start_nanos,
            txn.timestamp_preload_end_nanos,
            txn.timestamp_start_nanos,
        ] {
            if ts > 0 {
                start = start.min(ts);
            }
        }
        for ts in [
            txn.timestamp_mb_end_nanos,
            txn.timestamp_end_nanos,
            txn.timestamp_load_end_nanos,
        ] {
            if ts > 0 {
                end = end.max(ts);
            }
        }
    }

    if start == i64::MAX {
        start = 0;
    }
    (start, end.max(start))
}

fn nano_str(nanos: i64) -> String {
    nanos.to_string()
}

#[cfg(test)]
mod tests {
    use {super::*, crate::gui::metrics::GuiTxnRecord, solana_signature::Signature};

    #[test]
    fn query_transactions_response_shape() {
        let mut history = SlotTxnHistory::new(1_000, 2_000);
        history.txns.push(GuiTxnRecord {
            signature: Signature::default(),
            compute_units_requested: 42,
            bank_idx: 1,
            timestamp_preload_end_nanos: 1_100,
            timestamp_start_nanos: 1_200,
            timestamp_load_end_nanos: 1_300,
            timestamp_end_nanos: 1_400,
            landed: true,
            ..GuiTxnRecord::default()
        });

        let json = format_query_transactions_response(123, 3, Some(&history), false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["topic"], "slot");
        assert_eq!(value["key"], "query_transactions");
        assert_eq!(value["id"], 3);
        assert_eq!(value["value"]["publish"]["slot"], 123);
        assert!(value["value"]["transactions"].is_object());
        assert_eq!(
            value["value"]["transactions"]["txn_compute_units_requested"][0],
            42
        );
    }

    #[test]
    fn active_leader_returns_null_transactions() {
        let json = format_query_transactions_response(123, 3, None, true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value["value"]["transactions"].is_null());
        assert_eq!(value["value"]["publish"]["level"], "incomplete");
    }
}
