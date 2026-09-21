use {
    crate::gui::{
        metrics::GuiTxnRecord,
        slot_txn::{assemble_gui_txn_batch_items, GuiTxnEvent, SlotTxnHistory},
        txn_report::gui_txn_record_from_batch_item,
    },
    solana_clock::Slot,
    std::collections::{HashMap, VecDeque},
};

const MAX_RETAINED_LEADER_SLOTS: usize = 4096;
const MAX_TXNS_PER_SLOT: usize = 65_536;

/// Retains leader-slot transaction histories for `slot.query_transactions`.
#[derive(Debug, Default)]
pub struct SlotTxnStore {
    histories: HashMap<Slot, SlotTxnHistory>,
    insertion_order: VecDeque<Slot>,
}

const SLOT_RANKINGS_LIMIT: usize = 3;

#[derive(Debug, Default)]
pub struct SlotRankingsTotals {
    pub largest_fees: Vec<(Slot, u64)>,
    pub largest_tips: Vec<(Slot, u64)>,
    pub largest_rewards: Vec<(Slot, u64)>,
}

impl SlotTxnStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle_event(&mut self, event: GuiTxnEvent) {
        match event {
            GuiTxnEvent::TxnBatch(payload) => {
                let slot = payload.slot;
                let items = assemble_gui_txn_batch_items(&payload);
                if items.is_empty() {
                    return;
                }
                // One microblock id per GUI batch so frontend "X of Y" / post-execute
                // sibling bounds stay within this consume/bundle emit, not the whole slot.
                let microblock_idx = self.ensure_history(slot).alloc_microblock_idx();
                for (index, item) in items.into_iter().enumerate() {
                    let gui_timestamps = payload
                        .gui_timestamps_per_tx
                        .get(index)
                        .copied()
                        .unwrap_or_default();
                    let mut record =
                        gui_txn_record_from_batch_item(&item, &gui_timestamps);
                    record.microblock_idx = microblock_idx;
                    self.append_txn(slot, record);
                }
            }
        }
    }

    pub fn get_queryable(
        &self,
        slot: Slot,
        active_leader_slot: Option<Slot>,
    ) -> Option<&SlotTxnHistory> {
        if active_leader_slot == Some(slot) {
            return None;
        }
        self.histories.get(&slot)
    }

    pub fn compute_rankings(&self) -> SlotRankingsTotals {
        use crate::gui::slot_query::publish_from_history;

        let mut fees = Vec::new();
        let mut tips = Vec::new();
        let mut rewards = Vec::new();
        for (&slot, history) in &self.histories {
            let publish = publish_from_history(slot, Some(history), false);
            let fee_total = publish
                .transaction_fee
                .unwrap_or(0)
                .saturating_add(publish.priority_fee.unwrap_or(0));
            let tip_total = publish.tips.unwrap_or(0);
            let reward_total = fee_total.saturating_add(tip_total);
            if fee_total > 0 {
                fees.push((slot, fee_total));
            }
            if tip_total > 0 {
                tips.push((slot, tip_total));
            }
            if reward_total > 0 {
                rewards.push((slot, reward_total));
            }
        }

        fees.sort_by_key(|&(_, value)| std::cmp::Reverse(value));
        tips.sort_by_key(|&(_, value)| std::cmp::Reverse(value));
        rewards.sort_by_key(|&(_, value)| std::cmp::Reverse(value));
        fees.truncate(SLOT_RANKINGS_LIMIT);
        tips.truncate(SLOT_RANKINGS_LIMIT);
        rewards.truncate(SLOT_RANKINGS_LIMIT);

        SlotRankingsTotals {
            largest_fees: fees,
            largest_tips: tips,
            largest_rewards: rewards,
        }
    }

    /// Records wall-clock slot start from the GUI leader transition hook.
    pub fn set_slot_start(&mut self, slot: Slot, start_timestamp_nanos: i64, ns_per_slot: u128) {
        let history = self.ensure_history(slot);
        if history.start_timestamp_nanos == 0 {
            history.start_timestamp_nanos = start_timestamp_nanos;
        }
        if history.target_end_timestamp_nanos == 0 {
            history.target_end_timestamp_nanos =
                nominal_slot_end_nanos(start_timestamp_nanos, ns_per_slot);
        }
    }

    /// Records actual wall-clock slot end when leadership moves to another slot.
    pub fn set_slot_end(&mut self, slot: Slot, end_timestamp_nanos: i64) {
        let history = self.ensure_history(slot);
        history.target_end_timestamp_nanos = end_timestamp_nanos;
    }

    fn ensure_history(&mut self, slot: Slot) -> &mut SlotTxnHistory {
        if self.histories.len() >= MAX_RETAINED_LEADER_SLOTS && !self.histories.contains_key(&slot)
        {
            self.evict_oldest();
        }

        let is_new = !self.histories.contains_key(&slot);
        if is_new {
            self.insertion_order.push_back(slot);
        }
        self.histories
            .entry(slot)
            .or_insert_with(|| SlotTxnHistory::new(0, 0))
    }

    fn append_txn(&mut self, slot: Slot, record: GuiTxnRecord) {
        if self.histories.len() >= MAX_RETAINED_LEADER_SLOTS && !self.histories.contains_key(&slot)
        {
            self.evict_oldest();
        }

        let history = self.ensure_history(slot);
        if history.txns.len() >= MAX_TXNS_PER_SLOT {
            return;
        }
        history.txns.push(record);
    }

    fn evict_oldest(&mut self) {
        while let Some(slot) = self.insertion_order.pop_front() {
            if self.histories.remove(&slot).is_some() {
                return;
            }
        }
    }
}

fn nominal_slot_end_nanos(start_timestamp_nanos: i64, ns_per_slot: u128) -> i64 {
    let ns_per_slot_i64 = ns_per_slot.min(u128::from(i64::MAX as u64)) as i64;
    start_timestamp_nanos.saturating_add(ns_per_slot_i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_slot_start_sets_nominal_end() {
        let mut store = SlotTxnStore::new();
        store.set_slot_start(10, 1_000, 500);
        let history = store.get_queryable(10, None).unwrap();
        assert_eq!(history.start_timestamp_nanos, 1_000);
        assert_eq!(history.target_end_timestamp_nanos, 1_500);
    }

    #[test]
    fn set_slot_end_overwrites_nominal_end() {
        let mut store = SlotTxnStore::new();
        store.set_slot_start(10, 1_000, 500);
        store.set_slot_end(10, 1_800);
        let history = store.get_queryable(10, None).unwrap();
        assert_eq!(history.target_end_timestamp_nanos, 1_800);
    }

    #[test]
    fn set_slot_start_does_not_clobber_existing_start() {
        let mut store = SlotTxnStore::new();
        store.set_slot_start(10, 1_000, 500);
        store.set_slot_start(10, 9_000, 500);
        let history = store.get_queryable(10, None).unwrap();
        assert_eq!(history.start_timestamp_nanos, 1_000);
    }
}
