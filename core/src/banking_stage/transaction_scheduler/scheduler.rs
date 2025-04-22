use std::collections::HashMap;

use crate::banking_stage::{transaction_scheduler::scheduler_common::Batches, CostTrackerChannels};
#[cfg(feature = "dev-context-only-utils")]
use qualifier_attr::qualifiers;
use solana_cost_model::cost_tracker::CostTracker;
use solana_pubkey::Pubkey;

use {
    super::{
        scheduler_common::SchedulingCommon, scheduler_error::SchedulerError,
        transaction_state::TransactionState, transaction_state_container::StateContainer,
    },
    solana_runtime_transaction::transaction_with_meta::TransactionWithMeta,
};

#[cfg_attr(feature = "dev-context-only-utils", qualifiers(pub))]
pub trait Scheduler<Tx: TransactionWithMeta> {
    /// Schedule transactions from `container`.
    /// pre-graph and pre-lock filters may be passed to be applied
    /// before specific actions internally.
    fn schedule<S: StateContainer<Tx>>(
        &mut self,
        container: &mut S,
        pre_graph_filter: impl Fn(&[&Tx], &mut [bool]),
        pre_lock_filter: impl Fn(&TransactionState<Tx>) -> PreLockFilterAction,
        batches: Option<&mut Batches<Tx>>,
        scheduler_info: Option<&mut SchedulerInfo>,
    ) -> Result<SchedulingSummary, SchedulerError>;

    /// Receive completed batches of transactions without blocking.
    /// Returns (num_transactions, num_retryable_transactions) on success.
    fn receive_completed(
        &mut self,
        container: &mut impl StateContainer<Tx>,
        cost_tracker_channels: Option<&mut CostTrackerChannels>,
    ) -> Result<(usize, usize, Vec<u64>), SchedulerError>;

    /// All schedulers should have access to the common context for shared
    /// implementation.
    fn scheduling_common_mut(&mut self) -> &mut SchedulingCommon<Tx>;

    // returns if txns are in flight
    #[allow(dead_code)]
    fn in_flight_txns(&mut self) -> bool;

    #[allow(dead_code)]
    fn in_flight_cus(&mut self) -> u64;

    #[allow(dead_code)]
    fn cleanup_at_slot_boundary(&mut self);

    #[allow(dead_code)]
    fn retry_tx_ids<S: StateContainer<Tx>>(&mut self, _container: &mut S);

    #[allow(dead_code)]
    fn refresh_if_needed(
        &mut self,
        _accts_limit_reached: &HashMap<Pubkey, u64, ahash::RandomState>,
    );

    #[allow(dead_code)]
    fn sync_cost_tracker(&mut self, _cost_tracker: &CostTracker);
}

/// Action to be taken by pre-lock filter.
#[cfg_attr(feature = "dev-context-only-utils", qualifiers(pub))]
pub enum PreLockFilterAction {
    /// Attempt to schedule the transaction.
    AttemptToSchedule,
}

/// Metrics from scheduling transactions.
#[derive(Default, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "dev-context-only-utils", qualifiers(pub))]
pub struct SchedulingSummary {
    /// Starting queue size
    pub starting_queue_size: usize,
    /// Starting buffer size (outstanding txs are not counted in queue)
    pub starting_buffer_size: usize,

    /// Number of transactions scheduled.
    pub num_scheduled: usize,
    /// Number of transactions that were not scheduled due to conflicts.
    pub num_unschedulable_conflicts: usize,
    /// Number of transactions that were skipped due to thread capacity.
    pub num_unschedulable_threads: usize,
    /// Number of transactions that were dropped due to filter.
    pub num_filtered_out: usize,
    /// Time spent filtering transactions
    pub filter_time_us: u64,
}

#[derive(Default)]
pub struct SchedulerInfo {
    pub unscheduled_on_cu_limit: bool,
    pub considered_tx_break: bool,
    pub num_cu_throttled: u64,
    pub num_considered: u64,
    pub schedulable_threads_empty_count: u64,
    pub container_empty_count: u64,
}
