use {
    super::alpenglow_timing::alpenglow_slot_progress,
    agave_votor::slot_clock::SharedAlpenglowSlotClock,
    agave_votor_messages::migration::MigrationStatus,
    solana_clock::{
        BankId, DEFAULT_TICKS_PER_SLOT, FORWARD_TRANSACTIONS_TO_LEADER_AT_SLOT_OFFSET,
        HOLD_TRANSACTIONS_SLOT_OFFSET, Slot,
    },
    solana_poh::poh_recorder::{LeaderState, PohRecorder, SharedLeaderState},
    solana_runtime::bank::Bank,
    std::sync::{Arc, RwLock},
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
    previous_bank_id: BankId,
    poh_recorder: Arc<RwLock<PohRecorder>>,
    migration_status: Arc<MigrationStatus>,
    alpenglow_slot_clock: SharedAlpenglowSlotClock,
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
        previous_bank_id: BankId,
        poh_recorder: Arc<RwLock<PohRecorder>>,
        migration_status: Arc<MigrationStatus>,
        alpenglow_slot_clock: SharedAlpenglowSlotClock,
    ) -> Self {
        Self {
            shared_leader_state,
            ticks_per_slot,
            bank_creation_time,
            previous_bank_id,
            poh_recorder,
            migration_status,
            alpenglow_slot_clock,
        }
    }

    /// Construct from PohRecorder plus Alpenglow clock sources (switching point only).
    pub fn from_poh_recorder(
        poh_recorder: &Arc<RwLock<PohRecorder>>,
        migration_status: Arc<MigrationStatus>,
        alpenglow_slot_clock: SharedAlpenglowSlotClock,
    ) -> Self {
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
            0,
            poh_recorder.clone(),
            migration_status,
            alpenglow_slot_clock,
        )
    }

    #[inline]
    pub(crate) fn is_alpenglow_enabled(&self) -> bool {
        self.migration_status.is_alpenglow_enabled()
    }

    #[inline]
    pub fn make_consume_or_forward_decision(&mut self) -> (BufferedPacketsDecision, bool, u64) {
        self.make_consume_or_forward_decision_at(Instant::now(), false)
    }

    #[inline]
    pub(crate) fn make_atomic_consume_or_forward_decision(
        &mut self,
    ) -> (BufferedPacketsDecision, bool, u64) {
        self.make_consume_or_forward_decision_at(Instant::now(), true)
    }

    fn make_consume_or_forward_decision_at(
        &mut self,
        now: Instant,
        require_atomic_bank: bool,
    ) -> (BufferedPacketsDecision, bool, u64) {
        let state = self.shared_leader_state.load();
        let slot = if self.migration_status.is_alpenglow_enabled() {
            if let Some(slot_info) = self.alpenglow_slot_clock.load() {
                alpenglow_slot_progress(
                    slot_info.slot,
                    now.saturating_duration_since(slot_info.started_at),
                    slot_info.slot_duration,
                )
                .0
            } else {
                state.tick_height() / self.ticks_per_slot
            }
        } else {
            state.tick_height() / self.ticks_per_slot
        };

        let mut switching_point = false;

        let decision = if let Some(bank) = state.working_bank() {
            if require_atomic_bank && !state.atomic_batches_enabled() {
                BufferedPacketsDecision::Hold
            } else {
                if bank.bank_id() != self.previous_bank_id {
                    self.previous_bank_id = bank.bank_id();

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
            }
        } else if state.bank_slot().is_some() {
            BufferedPacketsDecision::Hold
        } else if self.migration_status.is_alpenglow_enabled() {
            self.make_decision_alpenglow(&state, now, &mut switching_point)
        } else {
            self.make_decision_poh(&state, &mut switching_point)
        };
        (decision, switching_point, slot)
    }

    fn make_decision_alpenglow(
        &self,
        state: &LeaderState,
        now: Instant,
        switching_point: &mut bool,
    ) -> BufferedPacketsDecision {
        let Some((leader_slot, last_leader_slot)) = state.next_leader_slot_range() else {
            return BufferedPacketsDecision::Forward;
        };
        let Some(slot_info) = self.alpenglow_slot_clock.load() else {
            return BufferedPacketsDecision::Forward;
        };
        // PoH tick height no longer advances after Alpenglow activation.
        let (current_slot, _) = alpenglow_slot_progress(
            slot_info.slot,
            now.saturating_duration_since(slot_info.started_at),
            slot_info.slot_duration,
        );
        if current_slot > last_leader_slot {
            return BufferedPacketsDecision::Forward;
        }
        let slots_until_leader = leader_slot.saturating_sub(current_slot);
        if slots_until_leader < FORWARD_TRANSACTIONS_TO_LEADER_AT_SLOT_OFFSET {
            BufferedPacketsDecision::Hold
        } else if slots_until_leader < HOLD_TRANSACTIONS_SLOT_OFFSET {
            BufferedPacketsDecision::ForwardAndHold
        } else {
            *switching_point = self.check_switching_point_alpenglow(current_slot, leader_slot);
            BufferedPacketsDecision::Forward
        }
    }

    fn make_decision_poh(
        &self,
        state: &LeaderState,
        switching_point: &mut bool,
    ) -> BufferedPacketsDecision {
        if let Some(leader_first_tick_height) = state.leader_first_tick_height() {
            let current_tick_height = state.tick_height();
            let ticks_until_leader = leader_first_tick_height.saturating_sub(current_tick_height);
            if ticks_until_leader
                <= (FORWARD_TRANSACTIONS_TO_LEADER_AT_SLOT_OFFSET - 1) * DEFAULT_TICKS_PER_SLOT
            {
                BufferedPacketsDecision::Hold
            } else if ticks_until_leader < HOLD_TRANSACTIONS_SLOT_OFFSET * DEFAULT_TICKS_PER_SLOT {
                BufferedPacketsDecision::ForwardAndHold
            } else {
                *switching_point = self.check_switching_point_ticks();
                BufferedPacketsDecision::Forward
            }
        } else {
            BufferedPacketsDecision::Forward
        }
    }

    fn check_switching_point_ticks(&self) -> bool {
        self.would_be_leader(SWITCHING_OFFSET * DEFAULT_TICKS_PER_SLOT)
            && !self.would_be_leader((SWITCHING_OFFSET - 1) * DEFAULT_TICKS_PER_SLOT)
    }

    fn check_switching_point_alpenglow(&self, current_slot: Slot, next_leader_slot: Slot) -> bool {
        would_be_leader_within_slots(current_slot, next_leader_slot, SWITCHING_OFFSET)
            && !would_be_leader_within_slots(
                current_slot,
                next_leader_slot,
                SWITCHING_OFFSET.saturating_sub(1),
            )
    }

    pub fn would_be_leader(&self, within_next_n_ticks: u64) -> bool {
        if let Some(leader_first_tick_height) =
            self.shared_leader_state.load().leader_first_tick_height()
        {
            let tick_height = self.shared_leader_state.load().tick_height();
            tick_height + within_next_n_ticks >= leader_first_tick_height
                && tick_height <= leader_first_tick_height + (self.ticks_per_slot * 4)
        } else {
            false
        }
    }
}

fn would_be_leader_within_slots(
    current_slot: Slot,
    next_leader_slot: Slot,
    within_next_n_slots: u64,
) -> bool {
    let leader_window_end = next_leader_slot.saturating_add(4);
    current_slot.saturating_add(within_next_n_slots) >= next_leader_slot
        && current_slot <= leader_window_end
}

impl From<&Arc<RwLock<PohRecorder>>> for DecisionMaker {
    fn from(poh_recorder: &Arc<RwLock<PohRecorder>>) -> Self {
        // Tower-default clock sources; prefer `from_poh_recorder` when Alpenglow
        // migration status / slot clock are available.
        Self::from_poh_recorder(
            poh_recorder,
            Arc::new(MigrationStatus::default()),
            SharedAlpenglowSlotClock::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        solana_ledger::{
            blockstore::Blockstore, genesis_utils::create_genesis_config,
            get_tmp_ledger_path_auto_delete,
        },
        solana_poh::poh_recorder::{LeaderState, create_test_recorder},
        solana_runtime::bank::Bank,
    };

    fn consume_bank(bank: Arc<Bank>) -> BufferedPacketsDecision {
        BufferedPacketsDecision::Consume(BankStart {
            working_bank: bank,
            bank_creation_time: Arc::new(Instant::now()),
        })
    }

    fn test_poh_recorder(bank: Arc<Bank>) -> Arc<RwLock<PohRecorder>> {
        let ledger_path = get_tmp_ledger_path_auto_delete!();
        let blockstore = Blockstore::open(ledger_path.path())
            .expect("Expected to be able to open database ledger");
        let (_exit, poh_recorder, _controller, _recorder, _service, _entry_receiver) =
            create_test_recorder(bank, Arc::new(blockstore), None, None);
        poh_recorder
    }

    fn test_decision_maker(
        shared_leader_state: SharedLeaderState,
        bank: Arc<Bank>,
        migration_status: Arc<MigrationStatus>,
        alpenglow_slot_clock: SharedAlpenglowSlotClock,
    ) -> DecisionMaker {
        DecisionMaker::new(
            shared_leader_state,
            DEFAULT_TICKS_PER_SLOT,
            Arc::new(Instant::now()),
            0,
            test_poh_recorder(bank),
            migration_status,
            alpenglow_slot_clock,
        )
    }

    #[test]
    fn test_buffered_packet_decision_bank() {
        let bank = Arc::new(Bank::default_for_tests());
        assert!(consume_bank(bank).bank().is_some());
        assert!(BufferedPacketsDecision::Forward.bank().is_none());
        assert!(BufferedPacketsDecision::ForwardAndHold.bank().is_none());
        assert!(BufferedPacketsDecision::Hold.bank().is_none());
    }

    #[test]
    fn test_alpenglow_decision_with_stopped_ticks() {
        use std::time::Duration;

        let mut shared_leader_state = SharedLeaderState::new(0, None, Some((24, 27)));
        let clock = SharedAlpenglowSlotClock::default();
        let bank = Arc::new(Bank::default_for_tests());
        let mut decision_maker = test_decision_maker(
            shared_leader_state.clone(),
            bank,
            Arc::new(MigrationStatus::post_migration_status()),
            clock.clone(),
        );
        let started_at = Instant::now();
        let slot_duration = Duration::from_millis(200);

        // No clock observation yet.
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at, false)
                .0,
            BufferedPacketsDecision::Forward
        );

        clock.update(4, started_at, slot_duration);
        // Exactly twenty slots away: do not buffer yet.
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at, false)
                .0,
            BufferedPacketsDecision::Forward
        );
        // Progress within the slot does not change the buffering decision.
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at + slot_duration / 2, false)
                .0,
            BufferedPacketsDecision::Forward
        );
        // Time advances while tick height stays at zero.
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at + slot_duration, false)
                .0,
            BufferedPacketsDecision::ForwardAndHold
        );
        // A stale observation cannot advance past its leader window.
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at + Duration::from_secs(60), false)
                .0,
            BufferedPacketsDecision::ForwardAndHold
        );

        clock.update(20, started_at, slot_duration);
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at + slot_duration * 2, false)
                .0,
            BufferedPacketsDecision::ForwardAndHold
        );
        // Exactly one slot away.
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at + slot_duration * 3, false)
                .0,
            BufferedPacketsDecision::Hold
        );
        // The leader window has begun but no working bank is available.
        clock.update(24, started_at, slot_duration);
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at, false)
                .0,
            BufferedPacketsDecision::Hold
        );

        // Keep buffering through the final slot of our leader window.
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at + slot_duration * 3, false)
                .0,
            BufferedPacketsDecision::Hold
        );
        // A newer observation beyond our window must stop buffering.
        clock.update(28, started_at, slot_duration);
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at, false)
                .0,
            BufferedPacketsDecision::Forward
        );

        shared_leader_state.store(Arc::new(LeaderState::new(None, 0, None, None)));
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at, false)
                .0,
            BufferedPacketsDecision::Forward
        );

        // Atomic batches wait until the optimistic parent is confirmed, even after migration.
        shared_leader_state.store(Arc::new(LeaderState::new_with_atomic_batches_enabled(
            Some(Arc::new(Bank::default_for_tests())),
            0,
            None,
            None,
            false,
        )));
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at, true)
                .0,
            BufferedPacketsDecision::Hold
        );
        shared_leader_state.load().enable_atomic_batches();
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at, true)
                .0,
            BufferedPacketsDecision::Consume(_)
        );
        shared_leader_state.set_bank_replacement();
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at, false)
                .0,
            BufferedPacketsDecision::Hold
        );

        // A working bank takes precedence over timing and schedule information.
        shared_leader_state.store(Arc::new(LeaderState::new(
            Some(Arc::new(Bank::default_for_tests())),
            0,
            None,
            None,
        )));
        assert_matches!(
            decision_maker
                .make_consume_or_forward_decision_at(started_at, false)
                .0,
            BufferedPacketsDecision::Consume(_)
        );
    }

    #[test]
    fn test_make_consume_or_forward_decision() {
        let genesis_config = create_genesis_config(2).genesis_config;
        let (bank, _bank_forks) = Bank::new_with_bank_forks_for_tests(&genesis_config);

        let mut shared_leader_state = SharedLeaderState::new(0, None, None);

        // Before migration, decisions must use ticks even if a clock is populated.
        let clock = SharedAlpenglowSlotClock::default();
        clock.update(100, Instant::now(), std::time::Duration::from_millis(200));
        let mut decision_maker = test_decision_maker(
            shared_leader_state.clone(),
            bank.clone(),
            Arc::new(MigrationStatus::default()),
            clock,
        );

        // No active bank, no leader first tick height.
        assert_matches!(
            decision_maker.make_consume_or_forward_decision().0,
            BufferedPacketsDecision::Forward
        );
        shared_leader_state.store(Arc::new(LeaderState::new_with_atomic_batches_enabled(
            None, 0, None, None, false,
        )));
        assert_matches!(
            decision_maker.make_atomic_consume_or_forward_decision().0,
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
            decision_maker.make_atomic_consume_or_forward_decision().0,
            BufferedPacketsDecision::Consume(_)
        );

        shared_leader_state.store(Arc::new(LeaderState::new_with_atomic_batches_enabled(
            Some(bank.clone()),
            0,
            None,
            None,
            false,
        )));
        assert_matches!(
            decision_maker.make_consume_or_forward_decision().0,
            BufferedPacketsDecision::Consume(_)
        );
        assert_matches!(
            decision_maker.make_atomic_consume_or_forward_decision().0,
            BufferedPacketsDecision::Hold
        );

        shared_leader_state.set_bank_replacement();
        assert!(matches!(
            decision_maker.make_consume_or_forward_decision().0,
            BufferedPacketsDecision::Hold
        ));
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

            let decision = decision_maker.make_consume_or_forward_decision().0;
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

            let decision = decision_maker.make_consume_or_forward_decision().0;
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
        let decision = decision_maker.make_consume_or_forward_decision().0;
        assert!(
            matches!(decision, BufferedPacketsDecision::Forward),
            "next_leader_slot: {next_leader_slot}",
        );
    }

    #[test]
    fn test_alpenglow_switching_point_uses_slot_clock() {
        let next_leader_slot = 100u64;
        let mut shared_leader_state = SharedLeaderState::new(0, None, None);
        shared_leader_state.store(Arc::new(LeaderState::new(
            None,
            0,
            Some(next_leader_slot * DEFAULT_TICKS_PER_SLOT),
            Some((next_leader_slot, next_leader_slot + 4)),
        )));

        let migration_status = Arc::new(MigrationStatus::post_migration_status());
        let alpenglow_slot_clock = SharedAlpenglowSlotClock::default();
        // At next_leader - 26: switching edge should fire.
        let edge_slot = next_leader_slot - SWITCHING_OFFSET;
        alpenglow_slot_clock.update(edge_slot, Instant::now(), Duration::from_secs(400));

        let bank = Arc::new(Bank::default_for_tests());
        let mut decision_maker = test_decision_maker(
            shared_leader_state.clone(),
            bank,
            migration_status,
            alpenglow_slot_clock.clone(),
        );

        let (decision, switching_point, _) = decision_maker.make_consume_or_forward_decision();
        assert!(matches!(decision, BufferedPacketsDecision::Forward));
        assert!(switching_point);

        // One slot later (next_leader - 25): still Forward, but not the edge.
        alpenglow_slot_clock.update(
            next_leader_slot - SWITCHING_OFFSET + 1,
            Instant::now(),
            Duration::from_secs(400),
        );
        let (decision, switching_point, _) = decision_maker.make_consume_or_forward_decision();
        assert!(matches!(decision, BufferedPacketsDecision::Forward));
        assert!(!switching_point);
    }
}
