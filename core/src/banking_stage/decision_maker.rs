use {
    solana_clock::{
        DEFAULT_TICKS_PER_SLOT, FORWARD_TRANSACTIONS_TO_LEADER_AT_SLOT_OFFSET,
        HOLD_TRANSACTIONS_SLOT_OFFSET,
    },
    solana_poh::poh_recorder::{PohRecorder, SharedLeaderState},
    solana_runtime::bank::Bank,
    solana_unified_scheduler_pool::{BankingStageMonitor, BankingStageStatus},
    std::sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        Arc, RwLock,
    },
    std::time::Instant,
};

const SWITCHING_OFFSET: u64 = 26;

#[derive(Debug, Clone)]
#[repr(C)]
pub struct BankStart {
    pub working_bank: Arc<Bank>,
    pub bank_creation_time: Arc<Instant>,
}

#[derive(Debug, Clone)]
#[repr(C)]
pub enum BufferedPacketsDecision {
    Consume(BankStart),
    Forward,
    ForwardAndHold,
    Hold,
}

impl PartialEq for BufferedPacketsDecision {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                BufferedPacketsDecision::Consume(bank_start1),
                BufferedPacketsDecision::Consume(bank_start2),
            ) => bank_start1.working_bank.slot() == bank_start2.working_bank.slot(),
            (BufferedPacketsDecision::Forward, BufferedPacketsDecision::Forward) => true,
            (BufferedPacketsDecision::ForwardAndHold, BufferedPacketsDecision::ForwardAndHold) => {
                true
            }
            (BufferedPacketsDecision::Hold, BufferedPacketsDecision::Hold) => true,
            _ => false,
        }
    }
}

impl BufferedPacketsDecision {
    /// Returns the `Bank` if the decision is `Consume`. Otherwise, returns `None`.
    pub fn bank(&self) -> Option<&Arc<Bank>> {
        match self {
            Self::Consume(bank_start) => Some(&bank_start.working_bank),
            _ => None,
        }
    }
}

#[derive(Clone)]
#[repr(C)]
pub struct DecisionMaker {
    shared_leader_state: SharedLeaderState,
    ticks_per_slot: u64,
    bank_creation_time: Arc<Instant>,
    previous_bank_slot: u64,
    poh_recorder: Arc<RwLock<PohRecorder>>,
}

impl std::fmt::Debug for DecisionMaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionMaker").finish()
    }
}

impl DecisionMaker {
    pub fn new(
        shared_leader_state: SharedLeaderState,
        ticks_per_slot: u64,
        bank_creation_time: Arc<Instant>,
        previous_bank_slot: u64,
        poh_recorder: Arc<RwLock<PohRecorder>>,
    ) -> Self {
        Self {
            shared_leader_state,
            ticks_per_slot,
            bank_creation_time,
            previous_bank_slot,
            poh_recorder,
        }
    }

    pub(crate) fn make_consume_or_forward_decision(
        &mut self,
    ) -> (BufferedPacketsDecision, bool, u64) {
        let state = self.shared_leader_state.load();
        let slot = state.tick_height() / self.ticks_per_slot;

        let mut switching_point = false;

        let decision = if let Some(bank) = state.working_bank() {
            if bank.slot() != self.previous_bank_slot {
                self.previous_bank_slot = bank.slot();
                self.bank_creation_time = self
                    .poh_recorder
                    .read()
                    .ok()
                    .and_then(|poh| poh.working_bank.as_ref().map(|bank| bank.start.clone()))
                    .unwrap_or_else(|| Arc::new(Instant::now()));
            }
            BufferedPacketsDecision::Consume(BankStart {
                working_bank: bank.clone(),
                bank_creation_time: self.bank_creation_time.clone(),
            })
        } else if let Some(leader_first_tick_height) = state.leader_first_tick_height() {
            let current_tick_height = state.tick_height();
            let ticks_until_leader = leader_first_tick_height.saturating_sub(current_tick_height);
            if ticks_until_leader
                <= (FORWARD_TRANSACTIONS_TO_LEADER_AT_SLOT_OFFSET - 1) * DEFAULT_TICKS_PER_SLOT
            {
                BufferedPacketsDecision::Hold
            } else if ticks_until_leader < HOLD_TRANSACTIONS_SLOT_OFFSET * DEFAULT_TICKS_PER_SLOT {
                BufferedPacketsDecision::ForwardAndHold
            } else {
                switching_point = self.check_switching_point();

                BufferedPacketsDecision::Forward
            }
        } else {
            switching_point = self.check_switching_point();

            BufferedPacketsDecision::Forward
        };
        (decision, switching_point, slot)
    }

    fn check_switching_point(&self) -> bool {
        self.would_be_leader(SWITCHING_OFFSET * DEFAULT_TICKS_PER_SLOT)
            && !self.would_be_leader((SWITCHING_OFFSET - 1) * DEFAULT_TICKS_PER_SLOT)
    }

    pub fn would_be_leader(&self, within_next_n_ticks: u64) -> bool {
        if let Some(leader_first_tick_height) =
            self.shared_leader_state.load().leader_first_tick_height()
        {
            self.shared_leader_state.load().tick_height() + within_next_n_ticks
                >= leader_first_tick_height
                && self.shared_leader_state.load().tick_height()
                    <= leader_first_tick_height + (self.ticks_per_slot * 4)
        } else {
            false
        }
    }
}

impl From<&Arc<RwLock<PohRecorder>>> for DecisionMaker {
    fn from(poh_recorder: &Arc<RwLock<PohRecorder>>) -> Self {
        let bank_creation_time = poh_recorder
            .read()
            .ok()
            .and_then(|poh| poh.working_bank.as_ref().map(|bank| bank.start.clone()))
            .unwrap_or_else(|| Arc::new(Instant::now()));

        let poh_recorder_deref = poh_recorder.read().unwrap();
        Self::new(
            poh_recorder_deref.shared_leader_state(),
            poh_recorder_deref.ticks_per_slot(),
            bank_creation_time,
            0, // initial default value
            poh_recorder.clone(),
        )
    }
}

#[derive(Debug)]
pub(crate) struct DecisionMakerWrapper {
    is_exited: Arc<AtomicBool>,
    decision_maker: DecisionMaker,
}

impl DecisionMakerWrapper {
    pub(crate) fn new(is_exited: Arc<AtomicBool>, decision_maker: DecisionMaker) -> Self {
        Self {
            is_exited,
            decision_maker,
        }
    }
}

impl BankingStageMonitor for DecisionMakerWrapper {
    fn status(&mut self) -> BankingStageStatus {
        if self.is_exited.load(Relaxed) {
            BankingStageStatus::Exited
        } else if matches!(
            self.decision_maker.make_consume_or_forward_decision().0,
            BufferedPacketsDecision::Forward,
        ) {
            BankingStageStatus::Inactive
        } else {
            BankingStageStatus::Active
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*, solana_ledger::genesis_utils::create_genesis_config,
        solana_poh::poh_recorder::LeaderState, solana_runtime::bank::Bank,
    };

    #[test]
    fn test_buffered_packet_decision_bank() {
        let bank = Arc::new(Bank::default_for_tests());
        assert!(BufferedPacketsDecision::Consume(bank).bank().is_some());
        assert!(BufferedPacketsDecision::Forward.bank().is_none());
        assert!(BufferedPacketsDecision::ForwardAndHold.bank().is_none());
        assert!(BufferedPacketsDecision::Hold.bank().is_none());
    }

    #[test]
    fn test_make_consume_or_forward_decision() {
        let genesis_config = create_genesis_config(2).genesis_config;
        let (bank, _bank_forks) = Bank::new_no_wallclock_throttle_for_tests(&genesis_config);

        let mut shared_leader_state = SharedLeaderState::new(0, None, None);

        let decision_maker = DecisionMaker::new(
            shared_leader_state.clone(),
            0,
            Arc::new(Instant::now()),
            0,
            Arc::new(RwLock::new(PohRecorder::default())),
        );

        // No active bank, no leader first tick height.
        assert_matches!(
            decision_maker.make_consume_or_forward_decision(),
            BufferedPacketsDecision::Forward
        );

        // Active bank.
        shared_leader_state.store(Arc::new(LeaderState::new(
            Some(bank.clone()),
            0,
            None,
            None,
        )));
        assert_matches!(
            decision_maker.make_consume_or_forward_decision(),
            BufferedPacketsDecision::Consume(_)
        );
        shared_leader_state.store(Arc::new(LeaderState::new(None, 0, None, None)));

        // Will be leader shortly - Hold
        for next_leader_slot_offset in [0, 1].into_iter() {
            let next_leader_slot = bank.slot() + next_leader_slot_offset;
            shared_leader_state.store(Arc::new(LeaderState::new(
                None,
                0,
                Some(next_leader_slot * DEFAULT_TICKS_PER_SLOT),
                Some((next_leader_slot, next_leader_slot + 4)),
            )));

            let decision = decision_maker.make_consume_or_forward_decision();
            assert!(
                matches!(decision, BufferedPacketsDecision::Hold),
                "next_leader_slot_offset: {next_leader_slot_offset}",
            );
        }

        // Will be leader - ForwardAndHold
        for next_leader_slot_offset in [2, 19].into_iter() {
            let next_leader_slot = bank.slot() + next_leader_slot_offset;
            shared_leader_state.store(Arc::new(LeaderState::new(
                None,
                0,
                Some(next_leader_slot * DEFAULT_TICKS_PER_SLOT),
                Some((next_leader_slot, next_leader_slot + 4)),
            )));

            let decision = decision_maker.make_consume_or_forward_decision();
            assert!(
                matches!(decision, BufferedPacketsDecision::ForwardAndHold),
                "next_leader_slot_offset: {next_leader_slot_offset}",
            );
        }

        // Longer period until next leader - Forward
        let next_leader_slot = 20 + bank.slot();
        shared_leader_state.store(Arc::new(LeaderState::new(
            None,
            0,
            Some(next_leader_slot * DEFAULT_TICKS_PER_SLOT),
            Some((next_leader_slot, next_leader_slot + 4)),
        )));
        let decision = decision_maker.make_consume_or_forward_decision();
        assert!(
            matches!(decision, BufferedPacketsDecision::Forward),
            "next_leader_slot: {next_leader_slot}",
        );
    }
}
