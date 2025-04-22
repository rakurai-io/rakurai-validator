use {
    super::{transaction_scheduler::transaction_state_container::SharedBytes, LikeClusterInfo},
    crate::{
        banking_stage::{
            decision_maker::DecisionMaker,
            scheduler_messages::MaxAge,
            transaction_scheduler::{
                scheduler_controller::translate_decision_into_decision_state,
                transaction_state::TransactionState,
            },
            BufferedPacketsDecision, DecisionState, SchedulerError, SchedulerObj,
        },
        validator::TransactionStructure,
    },
    agave_reserved_account_keys::ReservedAccountKeys,
    agave_transaction_view::{
        resolved_transaction_view::ResolvedTransactionView,
        transaction_view::SanitizedTransactionView,
    },
    anchor_lang::AccountDeserialize,
    crossbeam_channel::{Receiver, Sender},
    rakurai_activation::{
        sdk::{
            derive_activation_account_address,
            derive_config_account_address as derive_activation_config_account_address,
        },
        state::{RakuraiActivationAccount, RakuraiActivationConfigAccount},
    },
    reward_distribution::{
        sdk::{
            derive_config_account_address, derive_reward_collection_account_address,
            instruction::{
                initialize_reward_collection_account_ix, transfer_staker_rewards_ix,
                InitializeRewardCollectionAccountAccounts, InitializeRewardCollectionAccountArgs,
                TransferStakerRewardsAccounts, TransferStakerRewardsArgs,
            },
        },
        state::RewardCollectionAccount,
    },
    solana_account::{AccountSharedData, ReadableAccount},
    solana_clock::Slot,
    solana_cost_model::cost_tracker::CostTracker,
    solana_hash::Hash,
    solana_ledger::blockstore::Blockstore,
    solana_message::{Message, SimpleAddressLoader},
    solana_pubkey::Pubkey,
    solana_runtime::{bank::Bank, bank_forks::BankForks},
    solana_runtime_transaction::{
        runtime_transaction::RuntimeTransaction, transaction_meta::StaticMeta,
    },
    solana_sdk_ids::system_program,
    solana_signature::Signature,
    solana_signer::Signer,
    solana_svm_transaction::svm_message::SVMMessage,
    solana_transaction::{
        sanitized::{MessageHash, SanitizedTransaction},
        versioned::VersionedTransaction,
        Transaction,
    },
    solana_transaction_status::RewardType,
    std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
            Arc, RwLock,
        },
        time::Duration,
        u64,
    },
};

#[allow(dead_code)]
#[derive(Clone)]
pub struct LatestBankPair {
    pub root_bank: Arc<Bank>,
    pub working_bank: Arc<Bank>,
}

impl LatestBankPair {
    pub fn new(root_bank: Arc<Bank>, working_bank: Arc<Bank>) -> Self {
        Self {
            root_bank,
            working_bank,
        }
    }
}
enum AbstractTransaction {
    Sdk(RuntimeTransaction<SanitizedTransaction>),
    View(RuntimeTransaction<ResolvedTransactionView<SharedBytes>>),
}

impl AbstractTransaction {
    fn signature(&self) -> &Signature {
        match self {
            AbstractTransaction::Sdk(tx) => tx.signatures().first().unwrap(),
            AbstractTransaction::View(tx) => tx.signatures().first().unwrap(),
        }
    }

    fn message_hash(&self) -> &Hash {
        match self {
            AbstractTransaction::Sdk(tx) => tx.message_hash(),
            AbstractTransaction::View(tx) => tx.message_hash(),
        }
    }

    fn recent_blockhash(&self) -> &Hash {
        match self {
            AbstractTransaction::Sdk(tx) => tx.recent_blockhash(),
            AbstractTransaction::View(tx) => tx.recent_blockhash(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TxnsHistory {
    pub message_hash: Hash,
    pub blockhash: Hash,
    pub send_slot: u64,
    pub rewards: u64,
}

#[derive(Debug, Clone)]
#[repr(C)]
pub struct RewardDistributionConfig {
    pub rakurai_activation_program_id: Pubkey,
    pub reward_distribution_program_id: Pubkey,
    pub rewards_merkle_root_authority: Pubkey,
    pub vote_account: Pubkey,
}

impl Default for RewardDistributionConfig {
    fn default() -> Self {
        Self {
            rakurai_activation_program_id: Pubkey::new_unique(),
            reward_distribution_program_id: Pubkey::new_unique(),
            rewards_merkle_root_authority: Pubkey::new_unique(),
            vote_account: Pubkey::new_unique(),
        }
    }
}

#[derive(PartialEq, Debug)]
pub enum RCAState {
    Initialized,
    NotInitalized,
    Pending,
}
pub struct RewardDistributor<T: LikeClusterInfo> {
    cluster_info: T,
    blockstore: Arc<Blockstore>,
    bank_forks: Arc<RwLock<BankForks>>,
    rca_state: RCAState,
    distribution_config: RewardDistributionConfig,
    shared_decision: (Arc<RwLock<DecisionState>>, Arc<AtomicBool>),
    high_priority_transaction_sender_sdk:
        Option<Sender<SchedulerObj<RuntimeTransaction<SanitizedTransaction>>>>,
    high_priority_transaction_sender_view:
        Option<Sender<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>>,
    txns_history: HashMap<Signature, TxnsHistory>,
    accumulated_reward: u64,
    transaction_struct: TransactionStructure,
    decision_maker: DecisionMaker,
    scheduler_request_receiver: Receiver<(Pubkey, Sender<Option<AccountSharedData>>)>,
    shared_block_cost_limit: Arc<AtomicU64>,
    shared_account_cost_limit: Arc<AtomicU64>,
    shared_block_cost: Arc<AtomicU64>,
    update_trigger_receiver: Receiver<()>,
    cost_tracker_sender: Sender<CostTracker>,
    shared_bank_update: Arc<RwLock<LatestBankPair>>,
    input_tx_signature_sender: Option<(Sender<String>, Arc<AtomicBool>)>,
}

impl<T: LikeClusterInfo> RewardDistributor<T> {
    pub fn new(
        cluster_info: T,
        blockstore: Arc<Blockstore>,
        bank_forks: Arc<RwLock<BankForks>>,
        distribution_config: RewardDistributionConfig,
        shared_decision: (Arc<RwLock<DecisionState>>, Arc<AtomicBool>),
        high_priority_transaction_sender_sdk: Option<
            Sender<SchedulerObj<RuntimeTransaction<SanitizedTransaction>>>,
        >,
        high_priority_transaction_sender_view: Option<
            Sender<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>,
        >,
        transaction_struct: TransactionStructure,
        decision_maker: DecisionMaker,
        scheduler_request_receiver: Receiver<(Pubkey, Sender<Option<AccountSharedData>>)>,
        shared_block_cost_limit: Arc<AtomicU64>,
        shared_account_cost_limit: Arc<AtomicU64>,
        shared_block_cost: Arc<AtomicU64>,
        update_trigger_receiver: Receiver<()>,
        cost_tracker_sender: Sender<CostTracker>,
        shared_bank_update: Arc<RwLock<LatestBankPair>>,
        input_tx_signature_sender: Option<(Sender<String>, Arc<AtomicBool>)>,
    ) -> Self {
        Self {
            cluster_info,
            blockstore,
            bank_forks,
            rca_state: RCAState::NotInitalized,
            distribution_config,
            shared_decision,
            high_priority_transaction_sender_sdk,
            high_priority_transaction_sender_view,
            txns_history: HashMap::new(),
            accumulated_reward: 0,
            transaction_struct,
            decision_maker,
            scheduler_request_receiver,
            shared_block_cost_limit,
            shared_account_cost_limit,
            shared_block_cost,
            update_trigger_receiver,
            cost_tracker_sender,
            shared_bank_update,
            input_tx_signature_sender,
        }
    }

    pub fn read_rewards(&self, slot: Slot) -> Option<u64> {
        let bank_forks_r = self.bank_forks.read().ok()?;

        let bank = bank_forks_r.banks().get(&slot)?;
        if !bank.is_frozen() {
            return None;
        }

        let cloned_bank = bank.clone_without_scheduler();
        let rewards = cloned_bank.rewards.read().ok()?;

        let total = rewards
            .iter()
            .filter_map(|(_, reward_info)| {
                (reward_info.reward_type == RewardType::Fee).then_some(reward_info.lamports)
            })
            .sum::<i64>();

        Some(total as u64)
    }

    fn create_transfer_rca_transaction(
        &mut self,
        total_rewards: u64,
        reward_account: Pubkey,
        bank: &Bank,
    ) -> Option<AbstractTransaction> {
        let account_shared_data = bank.get_account(&reward_account)?;
        let mut account_data = account_shared_data.data();
        let reward_collection_account =
            RewardCollectionAccount::try_deserialize(&mut account_data).ok()?;

        if reward_collection_account.validator_commission_bps == 10_000
            && reward_collection_account.rakurai_commission_bps == 0
        {
            self.accumulated_reward = 0;
            return None;
        }

        let instruction = transfer_staker_rewards_ix(
            self.distribution_config.reward_distribution_program_id,
            TransferStakerRewardsArgs { total_rewards },
            TransferStakerRewardsAccounts {
                reward_collection_account: reward_account,
                rakurai_commission_account: reward_collection_account.rakurai_commission_account,
                system_program: system_program::id(),
                signer: self.cluster_info.keypair().clone().pubkey(),
            },
        );
        let message = Message::new(
            &[instruction],
            Some(&self.cluster_info.keypair().clone().pubkey()),
        );
        let tx = Transaction::new(
            &[self.cluster_info.keypair().clone()],
            message,
            bank.confirmed_last_blockhash(),
        );
        self.create_runtime_transaction(tx)
    }

    pub fn get_reward_collection_pda_status(&mut self, bank: &Bank) -> Pubkey {
        let (pda, _) = derive_reward_collection_account_address(
            &self.distribution_config.reward_distribution_program_id,
            &self.distribution_config.vote_account,
            bank.epoch(),
        );

        match bank.get_account(&pda) {
            None => {
                self.rca_state = if self.rca_state != RCAState::Pending {
                    RCAState::NotInitalized
                } else {
                    RCAState::Pending
                }
            }
            Some(account) => {
                if account.owner() == &self.distribution_config.reward_distribution_program_id {
                    self.rca_state = RCAState::Initialized
                } else {
                    self.rca_state = RCAState::NotInitalized
                }
            }
        };
        pda
    }

    fn initialize_reward_collection_account_tx(&self, bank: &Bank) -> Option<AbstractTransaction> {
        let activation_config_account_pubkey = derive_activation_config_account_address(
            &self.distribution_config.rakurai_activation_program_id,
        )
        .0;
        let config_account_shared_data = bank.get_account(&activation_config_account_pubkey)?;
        let mut config_account_data = config_account_shared_data.data();
        let rakurai_activation_config =
            RakuraiActivationConfigAccount::try_deserialize(&mut config_account_data).ok()?;

        let activation_account_pubkey = derive_activation_account_address(
            &self.distribution_config.rakurai_activation_program_id,
            &self.cluster_info.keypair().clone().pubkey(),
        )
        .0;
        let account_shared_data = bank.get_account(&activation_account_pubkey)?;
        let mut account_data = account_shared_data.data();
        let rakurai_activation =
            RakuraiActivationAccount::try_deserialize(&mut account_data).ok()?;

        let (reward_collection_account, bump) = derive_reward_collection_account_address(
            &self.distribution_config.reward_distribution_program_id,
            &self.distribution_config.vote_account,
            bank.epoch(),
        );

        let instruction = initialize_reward_collection_account_ix(
            self.distribution_config.reward_distribution_program_id,
            InitializeRewardCollectionAccountArgs {
                merkle_root_upload_authority: self
                    .distribution_config
                    .rewards_merkle_root_authority,
                validator_commission_bps: rakurai_activation.validator_commission_bps,
                rakurai_commission_account: rakurai_activation_config
                    .block_builder_commission_account,
                rakurai_commission_bps: rakurai_activation.block_builder_commission_bps,
                bump,
            },
            InitializeRewardCollectionAccountAccounts {
                config: derive_config_account_address(
                    &self.distribution_config.reward_distribution_program_id,
                )
                .0,
                reward_collection_account,
                validator_vote_account: self.distribution_config.vote_account,
                signer: self.cluster_info.keypair().clone().pubkey(),
                system_program: system_program::id(),
            },
        );

        let message = Message::new(
            &[instruction],
            Some(&self.cluster_info.keypair().clone().pubkey()),
        );
        let tx = Transaction::new(
            &[self.cluster_info.keypair().clone()],
            message,
            bank.last_blockhash(),
        );

        self.create_runtime_transaction(tx)
    }

    fn create_runtime_transaction(&self, tx: Transaction) -> Option<AbstractTransaction> {
        match self.transaction_struct {
            TransactionStructure::Sdk => {
                let versioned_transaction = VersionedTransaction::from(tx);
                if let Ok(tx) = RuntimeTransaction::try_create(
                    versioned_transaction,
                    MessageHash::Compute,
                    None,
                    SimpleAddressLoader::Disabled,
                    &ReservedAccountKeys::empty_key_set(),
                ) {
                    Some(AbstractTransaction::Sdk(tx))
                } else {
                    None
                }
            }
            TransactionStructure::View => {
                let serialized_transaction = {
                    let transaction = VersionedTransaction::from(tx);
                    bincode::serialize(&transaction).unwrap()
                };
                let transaction = SanitizedTransactionView::try_new_sanitized(Arc::clone(
                    &Arc::new(serialized_transaction),
                ))
                .unwrap();

                let static_runtime_transaction = RuntimeTransaction::<
                    SanitizedTransactionView<SharedBytes>,
                >::try_from(
                    transaction, MessageHash::Compute, None
                )
                .unwrap();

                let dynamic_runtime_transaction =
                    RuntimeTransaction::<ResolvedTransactionView<SharedBytes>>::try_from(
                        static_runtime_transaction,
                        None,
                        &ReservedAccountKeys::empty_key_set(),
                    );

                if let Ok(tx) = dynamic_runtime_transaction {
                    Some(AbstractTransaction::View(tx))
                } else {
                    None
                }
            }
        }
    }

    fn check_txn_status(&mut self) {
        let bank_forks_r = self.bank_forks.read();
        if bank_forks_r.is_ok() {
            let working_bank = bank_forks_r.unwrap().working_bank();
            let current_slot = working_bank.slot();

            self.txns_history.retain(|_sig, history| {
                let is_root = self.blockstore.is_root(history.send_slot);
                if !is_root {
                    return true;
                }

                let within_range = current_slot <= history.send_slot + 150;
                if !within_range {
                    return true;
                }
                let stats_cache_r = working_bank.status_cache.read();
                let status = if stats_cache_r.is_ok() {
                    stats_cache_r.unwrap().get_status(
                        &history.message_hash,
                        &history.blockhash,
                        &working_bank.ancestors,
                    )
                } else {
                    None
                };

                let has_status = status.is_some();
                if has_status {
                    return false;
                } else {
                    return true;
                }
            });
        }
    }

    pub fn process_init_and_transfer(&mut self, working_bank: Arc<Bank>, reward_account: Pubkey) {
        if self.rca_state == RCAState::NotInitalized {
            if let Some(runtime_tx) = self.initialize_reward_collection_account_tx(&working_bank) {
                self.send_transaction(runtime_tx);
                self.rca_state = RCAState::Pending;
            }
        }

        if self.accumulated_reward > 0 && self.rca_state == RCAState::Initialized {
            if let Some(runtime_tx) = self.create_transfer_rca_transaction(
                self.accumulated_reward,
                reward_account,
                &working_bank,
            ) {
                self.txns_history.insert(
                    *runtime_tx.signature(),
                    TxnsHistory {
                        rewards: self.accumulated_reward,
                        message_hash: *runtime_tx.message_hash(),
                        send_slot: working_bank.slot(),
                        blockhash: *runtime_tx.recent_blockhash(),
                    },
                );
                self.accumulated_reward = 0;

                self.send_transaction(runtime_tx);
            }
        }
    }

    fn send_transaction(&self, runtime_tx: AbstractTransaction) {
        match self.transaction_struct {
            TransactionStructure::Sdk => {
                if let AbstractTransaction::Sdk(tx) = runtime_tx {
                    let transaction_state = TransactionState::new(tx, MaxAge::MAX, u64::MAX, 150);
                    if let Some(sender) = &self.high_priority_transaction_sender_sdk {
                        // -----------------------------------------------------------------------------
                        // TX Input Signature Reporting
                        //
                        // This block sends all incoming transaction signatures (`tx_in_signature`) to
                        // the HouseKeeper via `input_tx_signature_sender`. Each
                        // transaction in the batch is processed to extract its signature. If a
                        // transaction fails deserialization, a serialized packet fallback is used
                        // to still identify the transaction.
                        //
                        // See the "tx_io_check_readme.md" for details on how these
                        // tx_in_signature messages are recorded and analyzed:
                        //   <repo-root>/tx_io_check_readme.md
                        //
                        // Collecting tx_in_signature ensures end-to-end auditing of transaction
                        // entry into the scheduler, enabling detection of missing or censored
                        // transactions and providing full transparency of scheduler behavior.
                        // -----------------------------------------------------------------------------
                        if let Some((input_tx_signature_sender, exit)) =
                            &self.input_tx_signature_sender
                        {
                            if !exit.load(Relaxed) {
                                let _ = input_tx_signature_sender.try_send(
                                    transaction_state.transaction().signature().to_string(),
                                );
                            }
                        }

                        let _ = sender.send(SchedulerObj {
                            scheduler_work_load: vec![transaction_state],
                        });
                    }
                }
            }
            TransactionStructure::View => {
                if let AbstractTransaction::View(tx) = runtime_tx {
                    let transaction_state = TransactionState::new(tx, MaxAge::MAX, u64::MAX, 150);
                    if let Some(sender) = &self.high_priority_transaction_sender_view {
                        // -----------------------------------------------------------------------------
                        // TX Input Signature Reporting
                        //
                        // This block sends all incoming transaction signatures (`tx_in_signature`) to
                        // the HouseKeeper via `input_tx_signature_sender`. Each
                        // transaction in the batch is processed to extract its signature. If a
                        // transaction fails deserialization, a serialized packet fallback is used
                        // to still identify the transaction.
                        //
                        // See the "tx_io_check_readme.md" for details on how these
                        // tx_in_signature messages are recorded and analyzed:
                        //   <repo-root>/tx_io_check_readme.md
                        //
                        // Collecting tx_in_signature ensures end-to-end auditing of transaction
                        // entry into the scheduler, enabling detection of missing or censored
                        // transactions and providing full transparency of scheduler behavior.
                        // -----------------------------------------------------------------------------
                        if let Some((input_tx_signature_sender, exit)) =
                            &self.input_tx_signature_sender
                        {
                            if !exit.load(Relaxed) {
                                let _ = input_tx_signature_sender.try_send(
                                    transaction_state
                                        .transaction()
                                        .signatures()
                                        .first()
                                        .unwrap()
                                        .to_string(),
                                );
                            }
                        }
                        let _ = sender.send(SchedulerObj {
                            scheduler_work_load: vec![transaction_state],
                        });
                    }
                }
            }
        };
    }

    fn read_rewards_and_check_txn_history(
        &mut self,
        slot_rewards: &mut HashMap<Slot, u64>,
        buffered_slots: &mut Vec<u64>,
    ) {
        slot_rewards.retain(|slot, reward| {
            if self.blockstore.is_root(*slot) {
                self.accumulated_reward += *reward;
                false
            } else if self.blockstore.is_skipped(*slot) {
                false //remove from record if skipped | missing from ledger
            } else {
                true //do not remove from record if !(skipped | rooted)
            }
        });
        if self.rca_state == RCAState::Pending {
            self.rca_state = RCAState::NotInitalized
        }
        buffered_slots.retain(|slot| match self.read_rewards(*slot) {
            Some(reward) => {
                info!("read-rewards-slot={:?},reward={}", slot, reward);
                slot_rewards.insert(*slot, reward);
                false
            }
            None => {
                if self.blockstore.is_skipped(*slot) {
                    false
                } else {
                    true
                }
            }
        });

        if !self.txns_history.is_empty() {
            self.check_txn_status();
        }
    }

    fn is_slot_changed(slot: &u64, previous_slot: &mut u64) -> bool {
        if slot != previous_slot {
            *previous_slot = *slot;
            true
        } else {
            false
        }
    }

    pub fn run(mut self) -> Result<(), SchedulerError> {
        let mut decision;
        let mut slot_rewards: HashMap<Slot, u64> = HashMap::new();
        let mut buffered_slots = Vec::new();
        let mut prev_decision = BufferedPacketsDecision::Hold;
        let mut switching_point;
        // leader's last slot, used to detect the change in slot
        let mut last_leader_slot: u64 = 0;
        let mut turn_started: bool = false;
        let mut slot;
        let mut previous_slot = 0;

        // this is const of 5ms because make_consume_or_forward_decision updates its decision after every 5 ms
        // so polling make_consume_or_forward_decision at a higher frequency is not needed
        let timeout_ms = Duration::from_millis(2);
        loop {
            // Get the current decision and switching point flag from the decision maker
            (decision, switching_point, slot) =
                self.decision_maker.make_consume_or_forward_decision();

            if Self::is_slot_changed(&slot, &mut previous_slot) {
                if let Ok(bank_forks) = self.bank_forks.read() {
                    let root_bank = bank_forks.root_bank();
                    let working_bank = bank_forks.working_bank();

                    *self.shared_bank_update.write().unwrap() =
                        LatestBankPair::new(root_bank.clone(), working_bank.clone());
                }
            }

            // Update the switching point flag when in forwarding because it only changes during forwarding decision
            if let BufferedPacketsDecision::Forward = decision {
                self.shared_decision.1.store(switching_point, Relaxed);
            }

            if let Ok(_) = self.update_trigger_receiver.recv_timeout(timeout_ms) {
                // handle the response
                self.send_cost_tracker_update(&decision);
            }

            if let Ok((pubkey, acct_sender)) = self.scheduler_request_receiver.try_recv() {
                self.handle_acct_request(pubkey, acct_sender)?;
            }

            if let BufferedPacketsDecision::Consume(bank_start) = &decision {
                self.shared_block_cost.store(
                    bank_start
                        .working_bank
                        .read_cost_tracker()
                        .map(|cost_tracker_read_lock| cost_tracker_read_lock.block_cost())
                        .unwrap_or_else(|_| CostTracker::default().block_cost()),
                    Relaxed,
                );
            }

            // Only update decision state if it has changed
            if decision != prev_decision {
                prev_decision = decision.clone();

                let mut decision_state_lock = self.shared_decision.0.write().unwrap();
                *decision_state_lock = translate_decision_into_decision_state(&decision);

                if let BufferedPacketsDecision::Consume(bank_start) = &decision {
                    let new_leader_slot = decision.bank_start().map(|b| b.working_bank.slot());
                    if let Some(new_leader_slot) = new_leader_slot {
                        if new_leader_slot != last_leader_slot {
                            last_leader_slot = new_leader_slot;
                            buffered_slots.push(new_leader_slot);

                            if !turn_started {
                                let cost_tracker = bank_start
                                    .working_bank
                                    .read_cost_tracker()
                                    .map(|cost_tracker_read_lock| cost_tracker_read_lock.clone())
                                    .unwrap_or_else(|_| CostTracker::default().clone());
                                self.shared_block_cost_limit
                                    .store(cost_tracker.block_cost_limit(), Relaxed);
                                self.shared_account_cost_limit
                                    .store(cost_tracker.account_cost_limit(), Relaxed);
                                // Mark the turn as started
                                turn_started = true;
                            }
                        }
                    }
                }
            }

            match decision {
                BufferedPacketsDecision::ForwardAndHold => {
                    self.read_rewards_and_check_txn_history(&mut slot_rewards, &mut buffered_slots);

                    let bank_forks_r = self.bank_forks.read();
                    if bank_forks_r.is_ok() {
                        let current_slot = bank_forks_r.unwrap().working_bank().slot();
                        self.txns_history.retain(|_, history| {
                            if current_slot > history.send_slot + 150 {
                                self.accumulated_reward += history.rewards;
                                false
                            } else {
                                true
                            }
                        });
                    }
                    turn_started = false;
                }

                BufferedPacketsDecision::Consume(bank_start) => {
                    let working_bank = bank_start.working_bank;
                    let reward_account = self.get_reward_collection_pda_status(&working_bank);
                    if self.rca_state == RCAState::Pending {
                        continue;
                    }
                    self.process_init_and_transfer(working_bank, reward_account);
                }

                BufferedPacketsDecision::Forward => {
                    self.read_rewards_and_check_txn_history(&mut slot_rewards, &mut buffered_slots);
                    turn_started = false;
                }

                BufferedPacketsDecision::Hold => {
                    let bank_forks_r = self.bank_forks.read();
                    if bank_forks_r.is_ok() {
                        let working_bank = bank_forks_r.unwrap().working_bank();
                        let reward_account = self.get_reward_collection_pda_status(&working_bank);
                        info!(
                            "block_reward_distributor,decision=hold,reward_collection_account={},epoch={},rca_status={:?},pending_txns_count={:?}",
                            reward_account,
                            working_bank.epoch(),
                            self.rca_state,
                            self.txns_history.len()
                        );
                        if self.rca_state == RCAState::Pending {
                            continue;
                        }
                        self.process_init_and_transfer(working_bank, reward_account);
                    }
                }
            }
        }
    }

    fn send_cost_tracker_update(&self, decision: &BufferedPacketsDecision) {
        // Only proceed if we’re about to consume buffered packets and the turn hasn't started
        if let BufferedPacketsDecision::Consume(bank_start) = &decision {
            let _ = self.cost_tracker_sender.try_send(
                bank_start
                    .working_bank
                    .read_cost_tracker()
                    .map(|cost_tracker_read_lock| cost_tracker_read_lock.clone())
                    .unwrap_or_else(|_| CostTracker::default()),
            );
        }
    }

    fn handle_acct_request(
        &mut self,
        pubkey: Pubkey,
        acct_sender: Sender<Option<AccountSharedData>>,
    ) -> Result<(), SchedulerError> {
        let bank = self.bank_forks.read().unwrap().working_bank();
        let account = bank.get_account(&pubkey);
        return match acct_sender.try_send(account) {
            Ok(_) => Ok(()),
            Err(_) => Err(SchedulerError::DisconnectedSendChannel(
                "scheduler disconnected",
            )),
        };
    }
}
