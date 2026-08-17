use {
    super::{LikeClusterInfo, transaction_scheduler::transaction_state_container::SharedBytes},
    crate::banking_stage::{
        DecisionState, SchedulerError, SchedulerObj,
        decision_maker::{BufferedPacketsDecision, DecisionMaker},
        postpack_confirmation_config::load_cached_uuid_mev_share_groups,
        scheduler_messages::MaxAge,
        transaction_scheduler::{
            scheduler_controller::translate_decision_into_decision_state,
            transaction_state::TransactionState,
        },
        virtual_priority_config::{
            CachedUuidTipGroup, TipUuidDelta, load_cached_uuid_tip_groups, snapshot_group_balances,
            weighted_tip_deltas_from_balances,
        },
    },
    agave_reserved_account_keys::ReservedAccountKeys,
    agave_transaction_view::{
        resolved_transaction_view::ResolvedTransactionView,
        transaction_view::SanitizedTransactionView,
    },
    anchor_lang::{AccountDeserialize, prelude::Pubkey as AnchorPubkey},
    crossbeam_channel::Sender,
    jito_tip_distribution::{
        sdk::derive_tip_distribution_account_address,
        state::{ClaimStatus, TipDistributionAccount},
    },
    rakurai_activation::{
        sdk::{
            derive_activation_account_address,
            derive_config_account_address as derive_activation_config_account_address,
        },
        state::{RakuraiActivationAccount, RakuraiActivationConfigAccount},
    },
    rakurai_tip_manager::{
        RAKURAI_REVENUE_NAME, TipManagerConfigAccount,
        sdk::{
            derive_rakurai_tip_manager_config_account_address,
            derive_rakurai_tip_payment_account_pdas, derive_record_authority_address,
            instruction::{
                ChangeTipReceiverV2Accounts, ChangeTipReceiverV2Args, change_tip_receiver_v2_ix,
            },
        },
    },
    reward_distribution::{
        sdk::{
            derive_config_account_address, derive_mev_share_collection_account_address,
            derive_mev_share_collection_account_v1_address,
            derive_reward_collection_account_address, derive_tip_collection_account_address,
            derive_tip_collection_account_v1_address, derive_tips_and_mev_share_config_address,
            instruction::{
                InitializeRevenueShareAccountV1Accounts, InitializeRevenueShareAccountV1Args,
                InitializeRewardCollectionAccountArgs, InitializeRewardCollectionAccountV1Accounts,
                RecordRevenueArgs, RecordRevenueShareAccounts,
                TransferClientCommissionOnMevCommissionAccounts,
                TransferClientCommissionOnMevCommissionArgs, TransferStakerRewardsAccounts,
                TransferStakerRewardsArgs, UpdateEpochConvertedToBlockRewardAccounts,
                UpdateEpochConvertedToBlockRewardArgs, initialize_revenue_share_account_v1_ix,
                initialize_reward_collection_account_v1_ix, record_revenue_v1_ix,
                transfer_client_commission_on_mev_commission_ix, transfer_staker_rewards_ix,
                update_epoch_converted_to_block_reward_ix,
                update_epoch_converted_to_block_reward_v1_ix,
            },
        },
        state::{RevenueShareAccount, RevenueShareAccountV1, RewardCollectionAccount},
    },
    solana_account::ReadableAccount,
    solana_clock::Slot,
    solana_compute_budget_interface::ComputeBudgetInstruction,
    solana_gossip::cluster_info::ClusterInfo,
    solana_hash::Hash,
    solana_instruction::{AccountMeta, Instruction},
    solana_ledger::blockstore::Blockstore,
    solana_message::Message,
    solana_pubkey::Pubkey,
    solana_runtime::{
        bank::Bank,
        bank_forks::BankForks,
        leader_schedule_utils::{
            first_of_consecutive_leader_slots, last_of_consecutive_leader_slots,
        },
    },
    solana_runtime_transaction::{
        runtime_transaction::RuntimeTransaction, transaction_meta::TransactionMeta,
        transaction_with_meta::TransactionWithMeta,
    },
    solana_sdk_ids::system_program,
    solana_signature::Signature,
    solana_signer::Signer,
    solana_svm_timings::wallclock_timestamp_nanos,
    solana_svm_transaction::{svm_message::SVMStaticMessage, svm_transaction::SVMTransaction},
    solana_transaction::{Transaction, sanitized::MessageHash, versioned::VersionedTransaction},
    solana_transaction_status::RewardType,
    std::{
        collections::HashMap,
        sync::{
            Arc, RwLock,
            atomic::{AtomicBool, Ordering::Relaxed},
        },
        time::{Duration, Instant},
        u64,
    },
    thiserror::Error,
};

#[cfg(feature = "build_validator")]
use solana_runtime::leader_schedule_utils::leader_slot_index;

#[cfg(feature = "build_validator")]
use crate::banking_stage::rakurai_enabled;

#[cfg(feature = "build_validator")]
unsafe extern "C" {
    #[allow(improper_ctypes)]
    pub fn reset_rakurai();
}

#[derive(Clone, Debug)]
pub struct RakuraiOpTxn {
    pub txn: Option<RuntimeTransaction<ResolvedTransactionView<Arc<Vec<u8>>>>>,
    pub landed: bool,
    last_send: Instant,
}

impl Default for RakuraiOpTxn {
    fn default() -> Self {
        Self {
            txn: None,
            landed: false,
            last_send: Instant::now(),
        }
    }
}

impl RakuraiOpTxn {
    pub fn default() -> Self {
        Default::default()
    }

    /// Create a new pending transaction
    pub fn txn(&mut self, txn: RuntimeTransaction<ResolvedTransactionView<Arc<Vec<u8>>>>) {
        self.txn = Some(txn);
        self.last_send = Instant::now();
    }

    /// Reset state
    pub fn reset(&mut self) {
        self.txn = None;
        self.landed = false;
    }

    /// Mark transaction as landed and release txn memory
    pub fn landed(&mut self) {
        self.txn = None;
        self.landed = true;
    }
}

/// Per-task in-flight txns sent from the reward-distributor consume loop.
/// Each task is its own transaction and is retried independently until it lands
/// or the leader turn ends.
struct RakuraiOpTxnSet {
    create_rca: RakuraiOpTxn,
    change_tip_receiver: RakuraiOpTxn,
    record_ix: RakuraiOpTxn,
    transfer_staker_rewards: RakuraiOpTxn,
    mev_commission: RakuraiOpTxn,
}

impl RakuraiOpTxnSet {
    fn new() -> Self {
        Self {
            create_rca: RakuraiOpTxn::default(),
            change_tip_receiver: RakuraiOpTxn::default(),
            record_ix: RakuraiOpTxn::default(),
            transfer_staker_rewards: RakuraiOpTxn::default(),
            mev_commission: RakuraiOpTxn::default(),
        }
    }

    fn reset(&mut self) {
        self.create_rca.reset();
        self.change_tip_receiver.reset();
        self.record_ix.reset();
        self.transfer_staker_rewards.reset();
        self.mev_commission.reset();
    }
}

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
    pub rakurai_tip_manager_program_id: Pubkey,
    pub rewards_merkle_root_authority: Pubkey,
    pub tip_distribution_program_id: Pubkey,
    pub vote_account: Pubkey,
}

impl Default for RewardDistributionConfig {
    fn default() -> Self {
        Self {
            rakurai_activation_program_id: Pubkey::new_unique(),
            reward_distribution_program_id: Pubkey::new_unique(),
            rakurai_tip_manager_program_id: Pubkey::new_unique(),
            rewards_merkle_root_authority: Pubkey::new_unique(),
            tip_distribution_program_id: Pubkey::new_unique(),
            vote_account: Pubkey::new_unique(),
        }
    }
}

#[derive(PartialEq, Debug)]
pub enum RakuraiCommissionOnMevStatus {
    NotDeducted,
    Deducted,
    SkippedThisEpoch,
    TransactionSent,
}

#[derive(Clone, Debug)]
struct TipTurnReport {
    first_slot: Slot,
    last_slot: Slot,
    baseline_slot: Slot,
    groups: Vec<CachedUuidTipGroup>,
    // Balances captured from the frozen baseline bank (state entering the turn).
    start_balances: HashMap<Pubkey, u64>,
    // Balances captured from the highest frozen leader bank of the turn, once available.
    end_balances: Option<HashMap<Pubkey, u64>>,
    end_captured_slot: Option<Slot>,
}

#[derive(Clone, Debug)]
struct PendingTipRevenueUpdate {
    uuid: String,
    uuid_name: [u8; 32],
    amount: u64,
    source_first_slot: Slot,
    source_last_slot: Slot,
}

/// Compute-unit limit used for block-reward conversion transactions. Paired with a
/// compute-unit price of `amount` micro-lamports/CU, this yields a priority fee of exactly
/// `amount` lamports (since `BLOCK_REWARD_CONVERSION_CU_LIMIT CU * (AMOUNT_MULTIPLICATION_FACTOR * amount ) µlamports/CU / 1_000_000 = amount lamports`).
const BLOCK_REWARD_CONVERSION_CU_LIMIT: u32 = 10_000;
const AMOUNT_MULTIPLICATION_FACTOR: u64 = 100;

pub struct RewardDistributor {
    cluster_info: Arc<ClusterInfo>,
    blockstore: Arc<Blockstore>,
    bank_forks: Arc<RwLock<BankForks>>,
    rakurai_commission_on_mev_commission_stats: RakuraiCommissionOnMevStatus,
    distribution_config: RewardDistributionConfig,
    shared_decision: (Arc<RwLock<DecisionState>>, Arc<AtomicBool>),
    high_priority_transaction_sender:
        Option<Sender<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>>,
    txns_history: HashMap<Signature, TxnsHistory>,
    accumulated_reward: u64,
    decision_maker: DecisionMaker,
    shared_bank_update: Arc<RwLock<LatestBankPair>>,
    input_tx_signature_sender: Option<(Sender<String>, Arc<AtomicBool>)>,
    #[allow(unused)]
    reset_rakurai: Arc<AtomicBool>,
    active_tip_turn: Option<TipTurnReport>,
    pending_tip_reports: Vec<TipTurnReport>,
    pending_tip_revenue_updates: Vec<PendingTipRevenueUpdate>,
    pending_tip_revenue_in_flight: Vec<PendingTipRevenueUpdate>,
}

impl RewardDistributor {
    pub fn new(
        cluster_info: Arc<ClusterInfo>,
        blockstore: Arc<Blockstore>,
        bank_forks: Arc<RwLock<BankForks>>,
        distribution_config: RewardDistributionConfig,
        shared_decision: (Arc<RwLock<DecisionState>>, Arc<AtomicBool>),
        high_priority_transaction_sender: Option<
            Sender<SchedulerObj<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>>>,
        >,
        decision_maker: DecisionMaker,
        shared_bank_update: Arc<RwLock<LatestBankPair>>,
        input_tx_signature_sender: Option<(Sender<String>, Arc<AtomicBool>)>,
        reset_rakurai: Arc<AtomicBool>,
    ) -> Self {
        Self {
            cluster_info,
            blockstore,
            bank_forks,
            rakurai_commission_on_mev_commission_stats: RakuraiCommissionOnMevStatus::NotDeducted,
            distribution_config,
            shared_decision,
            high_priority_transaction_sender,
            txns_history: HashMap::new(),
            accumulated_reward: 0,
            decision_maker,
            shared_bank_update,
            input_tx_signature_sender,
            reset_rakurai,
            active_tip_turn: None,
            pending_tip_reports: Vec::new(),
            pending_tip_revenue_updates: Vec::new(),
            pending_tip_revenue_in_flight: Vec::new(),
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

    pub fn warning_log(&self, msg: String) {
        let name: &'static str = "rakurai_warning";
        let datapoint = create_datapoint!(
            @point name,
            ("rakurai_warning_log", msg, String),
        );
        solana_metrics::submit(datapoint, log::Level::Warn);
    }

    fn transfer_mev_commission(
        &mut self,
        bank: &Bank,
        rca_pda: AnchorPubkey,
        tda_pda: AnchorPubkey,
    ) -> Result<Option<Instruction>, RewardDistributorError> {
        let (derive_mev_claim_status_pda_address, _bump) = Pubkey::find_program_address(
            &[
                ClaimStatus::SEED,
                &self.distribution_config.vote_account.to_bytes(),
                &tda_pda.to_bytes(),
            ],
            &self.distribution_config.tip_distribution_program_id,
        );
        match bank.get_account(&derive_mev_claim_status_pda_address) {
            None => {
                // Tip not distributed yet
                return Ok(None);
            }
            Some(account_shared_data) => {
                let mut account_data = account_shared_data.data();
                let claim_status_account = ClaimStatus::try_deserialize(&mut account_data)
                    .ok()
                    .unwrap();

                let account_shared_data = bank
                    .get_account(&Pubkey::new_from_array(rca_pda.as_array().clone()))
                    .ok_or(RewardDistributorError::RcaAccountNotFound)?;
                let mut account_data = account_shared_data.data();
                let reward_collection_account =
                    RewardCollectionAccount::try_deserialize(&mut account_data)
                        .ok()
                        .ok_or(RewardDistributorError::RcaDeserializationFailed)?;

                let reward_distribution_program_id = AnchorPubkey::from(
                    self.distribution_config
                        .reward_distribution_program_id
                        .as_array()
                        .clone(),
                );
                let system_program = AnchorPubkey::from(system_program::id().as_array().clone());
                let identity =
                    AnchorPubkey::from(self.cluster_info.id().clone().as_array().clone());

                let mut instruction = transfer_client_commission_on_mev_commission_ix(
                    reward_distribution_program_id,
                    TransferClientCommissionOnMevCommissionArgs {
                        mev_rewards: claim_status_account.amount,
                    },
                    TransferClientCommissionOnMevCommissionAccounts {
                        reward_collection_account: rca_pda,
                        client_commission_account: reward_collection_account
                            .client_commission_account,
                        system_program: system_program,
                        signer: identity,
                    },
                );
                let acct_metas: Vec<AccountMeta> = instruction
                    .accounts
                    .iter_mut()
                    .map(|acct| AccountMeta {
                        pubkey: Pubkey::from(acct.pubkey.as_array().clone()),
                        is_signer: acct.is_signer,
                        is_writable: acct.is_writable,
                    })
                    .collect();

                Ok(Some(Instruction::new_with_bytes(
                    self.distribution_config.reward_distribution_program_id,
                    &instruction.data,
                    acct_metas,
                )))
            }
        }
    }

    fn should_deduct_mev_commission(
        &mut self,
        bank: &Bank,
        threshold: u8,
    ) -> (bool, Option<AnchorPubkey>, Option<AnchorPubkey>) {
        let epoch = bank.epoch();
        let epoch_schedule = bank.epoch_schedule();
        let first_slot = epoch_schedule.get_first_slot_in_epoch(epoch);
        let last_slot = epoch_schedule.get_last_slot_in_epoch(epoch);
        let current_slot = bank.slot();

        // ---- Step 2: Check epoch progress
        let total_slots = last_slot - first_slot;
        let completed_slots = current_slot - first_slot;
        let completed_percent = (completed_slots as f64 / total_slots as f64) * 100.0;

        if completed_percent < threshold as f64 {
            trace!(
                "Not enough progress yet epoch: {epoch:}, completed_slots: {completed_slots:}, completed_percent: {completed_percent:}"
            );
            return (false, None, None);
        }

        let reward_distribution_program_id = AnchorPubkey::from(
            self.distribution_config
                .reward_distribution_program_id
                .as_array()
                .clone(),
        );
        let tip_distribution_program_id = AnchorPubkey::from(
            self.distribution_config
                .tip_distribution_program_id
                .as_array()
                .clone(),
        );
        let vote_account =
            AnchorPubkey::from(self.distribution_config.vote_account.as_array().clone());

        // ---- Step 2: Derive PDA addresses for last epoch
        let (rca_pda, _) = derive_reward_collection_account_address(
            &reward_distribution_program_id,
            &vote_account,
            epoch - 1,
        );
        let (tda_pda, _) = derive_tip_distribution_account_address(
            &tip_distribution_program_id,
            &vote_account,
            epoch - 1,
        );

        // ---- Step 3: Check RCA
        let rca_ok = bank
            .get_account(&Pubkey::from(rca_pda.as_array().clone()))
            .and_then(|account_shared_data| {
                let mut account_data = account_shared_data.data();
                RewardCollectionAccount::try_deserialize(&mut account_data).ok()
            })
            .map_or(false, |rca| match rca.client_mev_commission_deducted {
                None => {
                    self.rakurai_commission_on_mev_commission_stats =
                        RakuraiCommissionOnMevStatus::SkippedThisEpoch;
                    false
                }
                Some(0) => true,
                Some(_) => {
                    self.rakurai_commission_on_mev_commission_stats =
                        RakuraiCommissionOnMevStatus::Deducted;
                    false
                }
            });

        if !rca_ok {
            return (false, Some(rca_pda), Some(tda_pda));
        }

        // ---- Step 4: Check TDA
        let tda_ok = bank
            .get_account(&Pubkey::from(tda_pda.as_array().clone()))
            .and_then(|account_shared_data| {
                let mut account_data = account_shared_data.data();
                TipDistributionAccount::try_deserialize(&mut account_data).ok()
            })
            .map_or(false, |tda| {
                if tda.validator_commission_bps == 0 {
                    debug!("TDA has zero commission, skipping MEV commission deduction");
                    self.rakurai_commission_on_mev_commission_stats =
                        RakuraiCommissionOnMevStatus::SkippedThisEpoch;
                    false
                } else if tda.merkle_root.is_none() {
                    debug!("TDA has valid commission, proceeding with MEV commission deduction");
                    false
                } else {
                    debug!("TDA has valid commission, proceeding with MEV commission deduction");
                    true
                }
            });

        if !tda_ok {
            return (false, Some(rca_pda), Some(tda_pda));
        }

        (true, Some(rca_pda), Some(tda_pda))
    }

    fn create_transfer_rca_instruction(
        &mut self,
        total_rewards: u64,
        reward_account: Pubkey,
        bank: &Bank,
    ) -> Result<Option<Instruction>, RewardDistributorError> {
        let account_shared_data = bank
            .get_account(&reward_account)
            .ok_or(RewardDistributorError::RcaAccountNotFound)?;
        let mut account_data = account_shared_data.data();
        let reward_collection_account = RewardCollectionAccount::try_deserialize(&mut account_data)
            .ok()
            .ok_or(RewardDistributorError::RcaDeserializationFailed)?;

        if reward_collection_account.block_reward_commission_bps == 10_000
            && reward_collection_account.client_commission_bps == 0
        {
            self.accumulated_reward = 0;
            return Ok(None);
        }

        let reward_distribution_program_id = AnchorPubkey::from(
            self.distribution_config
                .reward_distribution_program_id
                .as_array()
                .clone(),
        );
        let reward_account = AnchorPubkey::from(reward_account.as_array().clone());
        let system_program = AnchorPubkey::from(system_program::id().as_array().clone());
        let identity = AnchorPubkey::from(self.cluster_info.id().clone().to_bytes());
        let mut instruction = transfer_staker_rewards_ix(
            reward_distribution_program_id,
            TransferStakerRewardsArgs { total_rewards },
            TransferStakerRewardsAccounts {
                reward_collection_account: reward_account,
                client_commission_account: reward_collection_account.client_commission_account,
                system_program,
                signer: identity,
            },
        );

        let acct_metas: Vec<AccountMeta> = instruction
            .accounts
            .iter_mut()
            .map(|acct| AccountMeta {
                pubkey: Pubkey::from(acct.pubkey.as_array().clone()),
                is_signer: acct.is_signer,
                is_writable: acct.is_writable,
            })
            .collect();

        Ok(Some(Instruction::new_with_bytes(
            self.distribution_config.reward_distribution_program_id,
            &instruction.data,
            acct_metas,
        )))
    }

    fn get_reward_collection_pda_status(&mut self, bank: &Bank) -> (bool, Pubkey) {
        let reward_distribution_program_id = AnchorPubkey::from(
            self.distribution_config
                .reward_distribution_program_id
                .as_array()
                .clone(),
        );
        let vote_account =
            AnchorPubkey::from(self.distribution_config.vote_account.as_array().clone());
        let (pda, _) = derive_reward_collection_account_address(
            &reward_distribution_program_id,
            &vote_account,
            bank.epoch(),
        );
        let pda = Pubkey::from(pda.as_array().clone());

        let rca_created = match bank.get_account(&pda) {
            None => false,
            Some(account) => {
                if account.owner() == &self.distribution_config.reward_distribution_program_id {
                    true
                } else {
                    false
                }
            }
        };
        (rca_created, pda)
    }

    fn initialize_reward_collection_account_instruction(
        &self,
        bank: &Bank,
    ) -> Result<Instruction, RewardDistributorError> {
        let rakurai_activation_program_id = AnchorPubkey::from(
            self.distribution_config
                .rakurai_activation_program_id
                .as_array()
                .clone(),
        );
        let activation_config_account_pubkey =
            derive_activation_config_account_address(&rakurai_activation_program_id).0;
        let config_account_shared_data = bank
            .get_account(&Pubkey::from(
                activation_config_account_pubkey.as_array().clone(),
            ))
            .ok_or(RewardDistributorError::RaaConfigAccountNotFound)?;
        let mut config_account_data = config_account_shared_data.data();
        let rakurai_activation_config =
            RakuraiActivationConfigAccount::try_deserialize(&mut config_account_data)
                .ok()
                .ok_or(RewardDistributorError::RaaConfigDeserializationFailed)?;

        let identity = AnchorPubkey::from(self.cluster_info.id().clone().as_array().clone());
        let activation_account_pubkey =
            derive_activation_account_address(&rakurai_activation_program_id, &identity).0;

        let account_shared_data = bank
            .get_account(&Pubkey::from(activation_account_pubkey.as_array().clone()))
            .ok_or(RewardDistributorError::RaaAccountNotFound)?;
        let mut account_data = account_shared_data.data();
        let rakurai_activation = RakuraiActivationAccount::try_deserialize(&mut account_data)
            .ok()
            .ok_or(RewardDistributorError::RaaDeserializationFailed)?;

        let reward_distribution_program_id = AnchorPubkey::from(
            self.distribution_config
                .reward_distribution_program_id
                .as_array()
                .clone(),
        );
        let vote_account =
            AnchorPubkey::from(self.distribution_config.vote_account.as_array().clone());
        let (reward_collection_account, bump) = derive_reward_collection_account_address(
            &reward_distribution_program_id,
            &vote_account,
            bank.epoch(),
        );

        let rewards_merkle_root_authority = AnchorPubkey::from(
            self.distribution_config
                .rewards_merkle_root_authority
                .as_array()
                .clone(),
        );
        let system_program = AnchorPubkey::from(system_program::id().as_array().clone());
        let mut instruction = initialize_reward_collection_account_v1_ix(
            reward_distribution_program_id,
            InitializeRewardCollectionAccountArgs {
                merkle_root_upload_authority: rewards_merkle_root_authority,
                block_reward_commission_bps: rakurai_activation.block_reward_commission_bps,
                client_commission_account: rakurai_activation_config.client_commission_account,
                client_commission_bps: rakurai_activation.client_commission_bps,
                bump,
            },
            InitializeRewardCollectionAccountV1Accounts {
                config: derive_config_account_address(&reward_distribution_program_id).0,
                reward_collection_account,
                validator_vote_account: vote_account,
                signer: identity,
                system_program,
                rakurai_activation_account: activation_account_pubkey,
            },
        );

        let acct_metas: Vec<AccountMeta> = instruction
            .accounts
            .iter_mut()
            .map(|acct| AccountMeta {
                pubkey: Pubkey::from(acct.pubkey.as_array().clone()),
                is_signer: acct.is_signer,
                is_writable: acct.is_writable,
            })
            .collect();

        Ok(Instruction::new_with_bytes(
            self.distribution_config.reward_distribution_program_id,
            &instruction.data,
            acct_metas,
        ))
    }

    fn create_runtime_transaction(
        &self,
        bank: &Bank,
        instructions: &[Instruction],
    ) -> Option<RuntimeTransaction<ResolvedTransactionView<SharedBytes>>> {
        let message = Message::new(&instructions, Some(&self.cluster_info.id()));
        let tx = Transaction::new(
            &[self.cluster_info.keypair().clone()],
            message,
            bank.last_blockhash(),
        );
        let enable_static_instruction_limit = bank
            .feature_set
            .is_active(&agave_feature_set::static_instruction_limit::ID);

        let serialized_transaction = {
            let transaction = VersionedTransaction::from(tx);
            bincode::serialize(&transaction).unwrap()
        };
        let sanitize_config = solana_runtime_transaction::sanitize_config::sanitize_config(
            enable_static_instruction_limit,
        );
        let transaction = SanitizedTransactionView::try_new_sanitized(
            Arc::clone(&Arc::new(serialized_transaction)),
            &sanitize_config,
        )
        .unwrap();

        let static_runtime_transaction =
            RuntimeTransaction::<SanitizedTransactionView<SharedBytes>>::try_new(
                transaction,
                MessageHash::Compute,
                None,
            )
            .ok()?;

        let dynamic_runtime_transaction =
            RuntimeTransaction::<ResolvedTransactionView<SharedBytes>>::try_new(
                static_runtime_transaction,
                None,
                &ReservedAccountKeys::empty_key_set(),
            );

        if dynamic_runtime_transaction.is_ok() {
            Some(dynamic_runtime_transaction.unwrap())
        } else {
            None
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

    pub fn change_tip_receiver_instruction(
        &mut self,
        bank: &Arc<Bank>,
        is_tip_receiver_changed: &mut bool,
    ) -> Result<Option<Vec<Instruction>>, RewardDistributorError> {
        // Get TipManager config Account
        let mut instructions = Vec::new();
        let rakurai_tip_manager_program_id = AnchorPubkey::from(
            self.distribution_config
                .rakurai_tip_manager_program_id
                .as_array()
                .clone(),
        );
        let identity = AnchorPubkey::from(self.cluster_info.id().clone().as_array().clone());
        let tip_manager_config_pda =
            derive_rakurai_tip_manager_config_account_address(&rakurai_tip_manager_program_id);
        let account_data = bank
            .get_account(&Pubkey::new_from_array(
                *tip_manager_config_pda.0.as_array(),
            ))
            .ok_or(RewardDistributorError::TipConfigAccountNotFound)?;
        let tip_manager_config = TipManagerConfigAccount::try_deserialize(&mut account_data.data())
            .ok()
            .ok_or(RewardDistributorError::TipConfigDeserializationFailed)?;
        let tip_accounts = derive_rakurai_tip_payment_account_pdas(&rakurai_tip_manager_program_id);

        // Check if TCA exist else create
        let vote_account =
            AnchorPubkey::from(self.distribution_config.vote_account.as_array().clone());
        let reward_distribution_program_id = AnchorPubkey::from(
            self.distribution_config
                .reward_distribution_program_id
                .as_array()
                .clone(),
        );
        let (tip_collection_account, tip_collection_account_bump) =
            derive_tip_collection_account_v1_address(
                &reward_distribution_program_id,
                &RAKURAI_REVENUE_NAME,
                &vote_account,
            );
        let tip_collection_pubkey = Pubkey::new_from_array(*tip_collection_account.as_array());

        if !bank
            .get_account(&tip_collection_pubkey)
            .is_some_and(|account| {
                account.owner() == &self.distribution_config.reward_distribution_program_id
            })
        {
            instructions.push(Self::anchor_ix_to_solana(
                self.distribution_config.reward_distribution_program_id,
                self.create_init_tip_collection_account_ix(
                    RAKURAI_REVENUE_NAME,
                    derive_record_authority_address(&rakurai_tip_manager_program_id).0,
                    identity,
                    vote_account,
                    reward_distribution_program_id,
                    tip_collection_account,
                    tip_collection_account_bump,
                ),
            ));
        }

        if tip_manager_config.validator_tip_receiver_account != tip_collection_account {
            *is_tip_receiver_changed = true;
            let mut instruction = change_tip_receiver_v2_ix(
                rakurai_tip_manager_program_id,
                ChangeTipReceiverV2Args,
                ChangeTipReceiverV2Accounts {
                    tip_manager_config: tip_manager_config_pda.0,
                    old_tip_receiver: tip_manager_config.validator_tip_receiver_account,
                    new_tip_receiver: tip_collection_account,
                    client_commission_account: tip_manager_config.client_commission_account,
                    rakurai_tip_account_0: tip_accounts[0].0,
                    rakurai_tip_account_1: tip_accounts[1].0,
                    rakurai_tip_account_2: tip_accounts[2].0,
                    rakurai_tip_account_3: tip_accounts[3].0,
                    rakurai_tip_account_4: tip_accounts[4].0,
                    rakurai_tip_account_5: tip_accounts[5].0,
                    rakurai_tip_account_6: tip_accounts[6].0,
                    rakurai_tip_account_7: tip_accounts[7].0,
                    signer: identity,
                    rakurai_activation_account: derive_activation_account_address(
                        &AnchorPubkey::from(
                            self.distribution_config
                                .rakurai_activation_program_id
                                .as_array()
                                .clone(),
                        ),
                        &identity,
                    )
                    .0,
                    reward_distribution_program: reward_distribution_program_id,
                    record_authority: derive_record_authority_address(
                        &rakurai_tip_manager_program_id,
                    )
                    .0,
                },
            );

            let acct_metas: Vec<AccountMeta> = instruction
                .accounts
                .iter_mut()
                .map(|acct| AccountMeta {
                    pubkey: Pubkey::from(acct.pubkey.as_array().clone()),
                    is_signer: acct.is_signer,
                    is_writable: acct.is_writable,
                })
                .collect();

            instructions.push(Instruction::new_with_bytes(
                self.distribution_config.rakurai_tip_manager_program_id,
                &instruction.data,
                acct_metas,
            ));
        } else {
            *is_tip_receiver_changed = false;
        }
        Ok(Some(instructions))
    }

    fn send_transaction(
        &self,
        txn_kind: String,
        bank: &Bank,
        runtime_tx: RuntimeTransaction<ResolvedTransactionView<SharedBytes>>,
    ) {
        let simulation_result = bank.simulate_transaction_unchecked(&runtime_tx, false);

        if let Err(err) = simulation_result.result {
            let txn_info = format!(
                "signature={},txn_info={:?}",
                runtime_tx.signature(),
                txn_kind
            );
            self.warning_log(format!(
                "reward_distributor epoch={},slot={},simulation=false,error={:?},txn={:?},vote_acc={},identity={}",
                bank.epoch(),
                bank.slot(),
                err,
                txn_info,
                self.distribution_config.vote_account.to_string(),
                self.cluster_info.keypair().pubkey().to_string(),
            ));
            return;
        }
        let transaction_state = TransactionState::new_with_ingress(
            runtime_tx,
            MaxAge::MAX,
            u64::MAX,
            150,
            wallclock_timestamp_nanos(),
            u32::from(std::net::Ipv4Addr::LOCALHOST),
        );
        if let Some(sender) = &self.high_priority_transaction_sender {
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
            if let Some((input_tx_signature_sender, exit)) = &self.input_tx_signature_sender {
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
        buffered_slots.retain(|slot| match self.read_rewards(*slot) {
            Some(reward) => {
                info!(
                    "reward_distributor read-rewards-slot={:?},reward={}",
                    slot, reward
                );
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

    /// Returns the highest frozen bank within the leader window `[first_slot, last_slot]`
    /// that is still retained in `bank_forks`. Used to capture end-of-turn tip balances
    /// before the root advances and prunes the leader banks.
    fn highest_frozen_bank_in_window(
        &self,
        first_slot: Slot,
        last_slot: Slot,
    ) -> Option<(Slot, Arc<Bank>)> {
        let bank_forks_r = self.bank_forks.read().ok()?;
        for slot in (first_slot..=last_slot).rev() {
            if let Some(bank) = bank_forks_r.banks().get(&slot) {
                if bank.is_frozen() {
                    return Some((slot, bank.clone_without_scheduler()));
                }
            }
        }
        None
    }

    /// Returns the frozen bank at exactly `slot` if it is still retained in `bank_forks`.
    fn frozen_bank_at(&self, slot: Slot) -> Option<Arc<Bank>> {
        let bank_forks_r = self.bank_forks.read().ok()?;
        let bank = bank_forks_r.banks().get(&slot)?;
        bank.is_frozen().then(|| bank.clone_without_scheduler())
    }

    fn anchor_ix_to_solana(
        program_id: Pubkey,
        instruction: anchor_lang::solana_program::instruction::Instruction,
    ) -> Instruction {
        let acct_metas: Vec<AccountMeta> = instruction
            .accounts
            .iter()
            .map(|acct| AccountMeta {
                pubkey: Pubkey::new_from_array(*acct.pubkey.as_array()),
                is_signer: acct.is_signer,
                is_writable: acct.is_writable,
            })
            .collect();

        Instruction::new_with_bytes(program_id, &instruction.data, acct_metas)
    }

    fn queue_tip_revenue_updates_from_deltas(
        &mut self,
        deltas: Vec<TipUuidDelta>,
        source_first_slot: Slot,
        source_last_slot: Slot,
    ) {
        for delta in deltas {
            if delta.amount == 0 || delta.uuid_name == [0u8; 32] {
                continue;
            }
            info!(
                "reward_distributor tip_turn_delta uuid={:?} weighted_delta_lamports={} \
                 source_first_slot={source_first_slot} source_last_slot={source_last_slot}",
                delta.uuid, delta.amount
            );
            self.pending_tip_revenue_updates
                .push(PendingTipRevenueUpdate {
                    uuid: delta.uuid,
                    uuid_name: delta.uuid_name,
                    amount: delta.amount,
                    source_first_slot,
                    source_last_slot,
                });
        }
    }

    fn append_pending_tip_revenue_instructions(
        &mut self,
        bank: &Bank,
        instructions: &mut Vec<Instruction>,
    ) -> Result<Vec<PendingTipRevenueUpdate>, RewardDistributorError> {
        if self.pending_tip_revenue_updates.is_empty() {
            return Ok(Vec::new());
        }

        let reward_distribution_program_id = AnchorPubkey::from(
            self.distribution_config
                .reward_distribution_program_id
                .as_array()
                .clone(),
        );
        let reward_distribution_program_pubkey =
            self.distribution_config.reward_distribution_program_id;
        let vote_account =
            AnchorPubkey::from(self.distribution_config.vote_account.as_array().clone());
        let identity = AnchorPubkey::from(self.cluster_info.id().clone().as_array().clone());

        let mut queued_updates = Vec::new();
        let pending = std::mem::take(&mut self.pending_tip_revenue_updates);

        for update in pending {
            if update.amount == 0 {
                continue;
            }

            let (tip_collection_account, tip_collection_account_bump) =
                derive_tip_collection_account_v1_address(
                    &reward_distribution_program_id,
                    &update.uuid_name,
                    &vote_account,
                );
            let tip_collection_pubkey = Pubkey::new_from_array(*tip_collection_account.as_array());

            if !bank
                .get_account(&tip_collection_pubkey)
                .is_some_and(|account| account.owner() == &reward_distribution_program_pubkey)
            {
                warn!(
                    "reward_distributor record_revenue initializing: tip collection account \
                    name={:?} (uuid={:?}) pda={tip_collection_pubkey}",
                    update.uuid, update.uuid_name
                );
                instructions.push(Self::anchor_ix_to_solana(
                    reward_distribution_program_pubkey,
                    self.create_init_tip_collection_account_ix(
                        update.uuid_name,
                        identity,
                        identity,
                        vote_account,
                        reward_distribution_program_id,
                        tip_collection_account,
                        tip_collection_account_bump,
                    ),
                ));
            }

            let anchor_ix = record_revenue_v1_ix(
                reward_distribution_program_id,
                RecordRevenueArgs {
                    amount: update.amount,
                },
                RecordRevenueShareAccounts {
                    revenue_share_account: tip_collection_account,
                    record_authority: identity,
                },
            );
            instructions.push(Self::anchor_ix_to_solana(
                reward_distribution_program_pubkey,
                anchor_ix,
            ));
            info!(
                "reward_distributor record_revenue queued uuid={:?} amount={} pda={tip_collection_pubkey} \
                 source_first_slot={} source_last_slot={}",
                update.uuid, update.amount, update.source_first_slot, update.source_last_slot,
            );
            queued_updates.push(update);
        }

        Ok(queued_updates)
    }

    fn restore_unlanded_tip_revenue_updates(&mut self) {
        if self.pending_tip_revenue_in_flight.is_empty() {
            return;
        }
        let count = self.pending_tip_revenue_in_flight.len();
        info!("reward_distributor record_revenue restoring {count} unlanded update(s) to pending");
        for update in &self.pending_tip_revenue_in_flight {
            info!(
                "reward_distributor record_revenue restoring uuid={:?} amount={} \
                 source_first_slot={} source_last_slot={}",
                update.uuid, update.amount, update.source_first_slot, update.source_last_slot,
            );
        }
        self.pending_tip_revenue_updates
            .append(&mut self.pending_tip_revenue_in_flight);
    }

    /// On each leader turn, scans every TCA ledger on-chain and sends a separate
    /// high-priority conversion transaction per eligible `(uuid, epoch)` entry.
    /// No in-memory pending state — the chain is always the source of truth.
    fn process_block_reward_conversions_from_chain(&mut self, bank: &Bank) {
        let current_epoch = bank.epoch();
        let reward_distribution_program_id = AnchorPubkey::from(
            self.distribution_config
                .reward_distribution_program_id
                .as_array()
                .clone(),
        );
        let reward_distribution_program_pubkey =
            self.distribution_config.reward_distribution_program_id;
        let vote_account =
            AnchorPubkey::from(self.distribution_config.vote_account.as_array().clone());
        let identity = AnchorPubkey::from(self.cluster_info.id().as_array().clone());

        let mut sent = 0usize;
        let mut waiting_claim = 0usize;
        let mut already_converted = 0usize;
        let mut tip_group_count = 0usize;
        let mut mev_share_group_count = 0usize;

        match load_cached_uuid_tip_groups(bank) {
            Some(groups) => {
                tip_group_count = groups.len();
                for group in groups {
                    let (tip_collection_account, _bump) = derive_tip_collection_account_address(
                        &reward_distribution_program_id,
                        &group.uuid_name,
                        &vote_account,
                    );
                    let (s, w, a) = self.process_revenue_share_block_reward_conversions(
                        bank,
                        current_epoch,
                        reward_distribution_program_id,
                        reward_distribution_program_pubkey,
                        vote_account,
                        identity,
                        &group.uuid,
                        "tip",
                        tip_collection_account,
                    );
                    sent += s;
                    waiting_claim += w;
                    already_converted += a;
                }
            }
            None => {
                info!(
                    "reward_distributor block_reward_conversion tip scan skipped: \
                     virtual priority config unavailable (current_epoch={current_epoch})"
                );
            }
        }

        match load_cached_uuid_mev_share_groups(bank) {
            Some(groups) => {
                mev_share_group_count = groups.len();
                for group in groups {
                    let (mev_share_collection_account, _bump) =
                        derive_mev_share_collection_account_address(
                            &reward_distribution_program_id,
                            &group.uuid_name,
                            &vote_account,
                        );
                    let (s, w, a) = self.process_revenue_share_block_reward_conversions(
                        bank,
                        current_epoch,
                        reward_distribution_program_id,
                        reward_distribution_program_pubkey,
                        vote_account,
                        identity,
                        &group.uuid,
                        "mev_share",
                        mev_share_collection_account,
                    );
                    sent += s;
                    waiting_claim += w;
                    already_converted += a;
                }
            }
            None => {
                info!(
                    "reward_distributor block_reward_conversion mev_share scan skipped: \
                     postpack confirmation config unavailable (current_epoch={current_epoch})"
                );
            }
        }

        info!(
            "reward_distributor block_reward_conversion scan complete current_epoch={current_epoch} \
             tip_uuid_groups={tip_group_count} mev_share_uuid_groups={mev_share_group_count} \
             txn_sent={sent} waiting_claim={waiting_claim} already_converted={already_converted}"
        );
    }

    /// Mirrors the legacy scan for TCAV1/MCAV1 revenue-share accounts (`REVENUE_SHARE_V1`).
    fn process_block_reward_conversions_from_chain_v1(&mut self, bank: &Bank) {
        let current_epoch = bank.epoch();
        let reward_distribution_program_id = AnchorPubkey::from(
            self.distribution_config
                .reward_distribution_program_id
                .as_array()
                .clone(),
        );
        let reward_distribution_program_pubkey =
            self.distribution_config.reward_distribution_program_id;
        let vote_account =
            AnchorPubkey::from(self.distribution_config.vote_account.as_array().clone());
        let identity = AnchorPubkey::from(self.cluster_info.id().as_array().clone());

        let mut sent = 0usize;
        let mut waiting_claim = 0usize;
        let mut already_converted = 0usize;
        let mut tip_group_count = 0usize;
        let mut mev_share_group_count = 0usize;

        match load_cached_uuid_tip_groups(bank) {
            Some(groups) => {
                tip_group_count = groups.len();
                for group in groups {
                    let (tip_collection_account, _bump) = derive_tip_collection_account_v1_address(
                        &reward_distribution_program_id,
                        &group.uuid_name,
                        &vote_account,
                    );
                    let (s, w, a) = self.process_revenue_share_block_reward_conversions_v1(
                        bank,
                        current_epoch,
                        reward_distribution_program_id,
                        reward_distribution_program_pubkey,
                        vote_account,
                        identity,
                        &group.uuid,
                        "tip",
                        tip_collection_account,
                    );
                    sent += s;
                    waiting_claim += w;
                    already_converted += a;
                }
            }
            None => {
                info!(
                    "reward_distributor block_reward_conversion_v1 tip scan skipped: \
                     virtual priority config unavailable (current_epoch={current_epoch})"
                );
            }
        }

        match load_cached_uuid_mev_share_groups(bank) {
            Some(groups) => {
                mev_share_group_count = groups.len();
                for group in groups {
                    let (mev_share_collection_account, _bump) =
                        derive_mev_share_collection_account_v1_address(
                            &reward_distribution_program_id,
                            &group.uuid_name,
                            &vote_account,
                        );
                    let (s, w, a) = self.process_revenue_share_block_reward_conversions_v1(
                        bank,
                        current_epoch,
                        reward_distribution_program_id,
                        reward_distribution_program_pubkey,
                        vote_account,
                        identity,
                        &group.uuid,
                        "mev_share",
                        mev_share_collection_account,
                    );
                    sent += s;
                    waiting_claim += w;
                    already_converted += a;
                }
            }
            None => {
                info!(
                    "reward_distributor block_reward_conversion_v1 mev_share scan skipped: \
                     postpack confirmation config unavailable (current_epoch={current_epoch})"
                );
            }
        }

        info!(
            "reward_distributor block_reward_conversion_v1 scan complete current_epoch={current_epoch} \
             tip_uuid_groups={tip_group_count} mev_share_uuid_groups={mev_share_group_count} \
             txn_sent={sent} waiting_claim={waiting_claim} already_converted={already_converted}"
        );
    }

    /// Scans one revenue-share PDA (TCA or MCA) and sends conversion txs for eligible ledger entries.
    /// Returns `(txn_sent, waiting_claim, already_converted)`.
    fn process_revenue_share_block_reward_conversions(
        &mut self,
        bank: &Bank,
        current_epoch: u64,
        reward_distribution_program_id: AnchorPubkey,
        reward_distribution_program_pubkey: Pubkey,
        vote_account: AnchorPubkey,
        identity: AnchorPubkey,
        uuid: &str,
        share_kind: &str,
        revenue_share_account_pda: AnchorPubkey,
    ) -> (usize, usize, usize) {
        let mut sent = 0usize;
        let mut waiting_claim = 0usize;
        let mut already_converted = 0usize;

        let revenue_share_pubkey = Pubkey::new_from_array(*revenue_share_account_pda.as_array());
        let Some(account) = bank.get_account(&revenue_share_pubkey) else {
            return (sent, waiting_claim, already_converted);
        };
        if account.owner() != &reward_distribution_program_pubkey {
            return (sent, waiting_claim, already_converted);
        }
        let Ok(revenue_share_account) = RevenueShareAccount::try_deserialize(&mut account.data())
        else {
            warn!(
                "reward_distributor block_reward_conversion scan: {share_kind} deserialize failed \
                 uuid={uuid:?} pda={revenue_share_pubkey}"
            );
            return (sent, waiting_claim, already_converted);
        };

        if !revenue_share_account.block_reward_conversion_enabled {
            return (sent, waiting_claim, already_converted);
        }

        for entry in &revenue_share_account.ledger.entries {
            if entry.epoch >= current_epoch {
                continue;
            }
            if entry.block_reward_converted {
                already_converted += 1;
                continue;
            }
            if entry.amount == 0 {
                continue;
            }

            if !entry.claimed {
                waiting_claim += 1;
                info!(
                    "reward_distributor block_reward_conversion waiting: kind={share_kind} \
                     uuid={uuid:?} epoch={} claimed=false amount={} pda={revenue_share_pubkey}",
                    entry.epoch, entry.amount
                );
                continue;
            }

            let total_amount = entry.amount;
            let commission_amount = if revenue_share_account.commission_bps == 0
                || revenue_share_account.name == RAKURAI_REVENUE_NAME
            {
                0
            } else {
                ((total_amount as u128)
                    .saturating_mul(revenue_share_account.commission_bps as u128)
                    / 10_000u128) as u64
            };
            let amount = total_amount.saturating_sub(commission_amount);
            if amount == 0 {
                info!(
                    "reward_distributor block_reward_conversion skip: zero validator amount \
                     kind={share_kind} uuid={uuid:?} epoch={} total_amount={total_amount} \
                     commission_bps={} pda={revenue_share_pubkey}",
                    entry.epoch, revenue_share_account.commission_bps
                );
                continue;
            }

            let convert_ix = Self::anchor_ix_to_solana(
                reward_distribution_program_pubkey,
                update_epoch_converted_to_block_reward_ix(
                    reward_distribution_program_id,
                    UpdateEpochConvertedToBlockRewardArgs { epoch: entry.epoch },
                    UpdateEpochConvertedToBlockRewardAccounts {
                        revenue_share_account: revenue_share_account_pda,
                        validator_vote_account: vote_account,
                        signer: identity,
                    },
                ),
            );

            let instructions = vec![
                ComputeBudgetInstruction::set_compute_unit_limit(BLOCK_REWARD_CONVERSION_CU_LIMIT),
                ComputeBudgetInstruction::set_compute_unit_price(
                    amount * AMOUNT_MULTIPLICATION_FACTOR,
                ),
                convert_ix,
            ];

            match self.create_runtime_transaction(bank, &instructions) {
                Some(runtime_tx) => {
                    let signature = runtime_tx.signature().clone();
                    info!(
                        "reward_distributor block_reward_conversion txn_sent kind={share_kind} \
                         uuid={uuid:?} epoch={} total_amount={total_amount} commission_bps={} \
                         priority_fee_lamports={amount} pda={revenue_share_pubkey} sig={signature}",
                        entry.epoch, revenue_share_account.commission_bps
                    );
                    self.send_transaction(
                        format!(
                            "block_reward_conversion=legacy,share_kind={},uuid={},revenue_pda={}",
                            share_kind, uuid, revenue_share_pubkey
                        ),
                        &bank,
                        runtime_tx,
                    );
                    sent += 1;
                }
                None => {
                    warn!(
                        "reward_distributor block_reward_conversion failed to build txn \
                         kind={share_kind} uuid={uuid:?} epoch={} pda={revenue_share_pubkey}",
                        entry.epoch
                    );
                }
            }
        }

        (sent, waiting_claim, already_converted)
    }

    /// Scans one V1 revenue-share PDA (TCAV1 or MCAV1) and sends conversion txs for
    /// eligible ledger entries. Returns `(txn_sent, waiting_claim, already_converted)`.
    fn process_revenue_share_block_reward_conversions_v1(
        &mut self,
        bank: &Bank,
        current_epoch: u64,
        reward_distribution_program_id: AnchorPubkey,
        reward_distribution_program_pubkey: Pubkey,
        vote_account: AnchorPubkey,
        identity: AnchorPubkey,
        uuid: &str,
        share_kind: &str,
        revenue_share_account_pda: AnchorPubkey,
    ) -> (usize, usize, usize) {
        let mut sent = 0usize;
        let mut waiting_claim = 0usize;
        let mut already_converted = 0usize;

        let revenue_share_pubkey = Pubkey::new_from_array(*revenue_share_account_pda.as_array());
        let Some(account) = bank.get_account(&revenue_share_pubkey) else {
            return (sent, waiting_claim, already_converted);
        };
        if account.owner() != &reward_distribution_program_pubkey {
            return (sent, waiting_claim, already_converted);
        }
        let Ok(revenue_share_account) = RevenueShareAccountV1::try_deserialize(&mut account.data())
        else {
            warn!(
                "reward_distributor block_reward_conversion_v1 scan: {share_kind} deserialize failed \
                 uuid={uuid:?} pda={revenue_share_pubkey}"
            );
            return (sent, waiting_claim, already_converted);
        };

        if !revenue_share_account.block_reward_conversion_enabled {
            return (sent, waiting_claim, already_converted);
        }

        for entry in &revenue_share_account.ledger.entries {
            if entry.epoch >= current_epoch {
                continue;
            }
            if entry.block_reward_converted {
                already_converted += 1;
                continue;
            }
            // V1 claimable / conversion base is settled funds (`transferred_amount`), not
            // recorded `amount` (which may differ under under/over-settle).
            if entry.transferred_amount == 0 {
                continue;
            }

            if !entry.claimed {
                waiting_claim += 1;
                info!(
                    "reward_distributor block_reward_conversion_v1 waiting: kind={share_kind} \
                     uuid={uuid:?} epoch={} claimed=false transferred_amount={} amount={} \
                     pda={revenue_share_pubkey}",
                    entry.epoch, entry.transferred_amount, entry.amount
                );
                continue;
            }

            let total_amount = entry.transferred_amount;
            let commission_amount = if revenue_share_account.commission_bps == 0
                || revenue_share_account.name == RAKURAI_REVENUE_NAME
            {
                0
            } else {
                ((total_amount as u128)
                    .saturating_mul(revenue_share_account.commission_bps as u128)
                    / 10_000u128) as u64
            };
            let amount = total_amount.saturating_sub(commission_amount);
            if amount == 0 {
                info!(
                    "reward_distributor block_reward_conversion_v1 skip: zero validator amount \
                     kind={share_kind} uuid={uuid:?} epoch={} transferred_amount={total_amount} \
                     commission_bps={} pda={revenue_share_pubkey}",
                    entry.epoch, revenue_share_account.commission_bps
                );
                continue;
            }

            let convert_ix = Self::anchor_ix_to_solana(
                reward_distribution_program_pubkey,
                update_epoch_converted_to_block_reward_v1_ix(
                    reward_distribution_program_id,
                    UpdateEpochConvertedToBlockRewardArgs { epoch: entry.epoch },
                    UpdateEpochConvertedToBlockRewardAccounts {
                        revenue_share_account: revenue_share_account_pda,
                        validator_vote_account: vote_account,
                        signer: identity,
                    },
                ),
            );

            let instructions = vec![
                ComputeBudgetInstruction::set_compute_unit_limit(BLOCK_REWARD_CONVERSION_CU_LIMIT),
                ComputeBudgetInstruction::set_compute_unit_price(
                    amount * AMOUNT_MULTIPLICATION_FACTOR,
                ),
                convert_ix,
            ];

            match self.create_runtime_transaction(bank, &instructions) {
                Some(runtime_tx) => {
                    let signature = runtime_tx.signature().clone();
                    info!(
                        "reward_distributor block_reward_conversion_v1 txn_sent kind={share_kind} \
                         uuid={uuid:?} epoch={} transferred_amount={total_amount} commission_bps={} \
                         priority_fee_lamports={amount} pda={revenue_share_pubkey} sig={signature}",
                        entry.epoch, revenue_share_account.commission_bps
                    );
                    self.send_transaction(
                        format!(
                            "block_reward_conversion=v1,share_kind={},uuid={},revenue_pda={}",
                            share_kind, uuid, revenue_share_pubkey
                        ),
                        &bank,
                        runtime_tx,
                    );
                    sent += 1;
                }
                None => {
                    warn!(
                        "reward_distributor block_reward_conversion_v1 failed to build txn \
                         kind={share_kind} uuid={uuid:?} epoch={} pda={revenue_share_pubkey}",
                        entry.epoch
                    );
                }
            }
        }

        (sent, waiting_claim, already_converted)
    }

    fn create_init_tip_collection_account_ix(
        &self,
        name: [u8; 32],
        record_authority: AnchorPubkey,
        identity: AnchorPubkey,
        vote_account: AnchorPubkey,
        reward_distribution_program_id: AnchorPubkey,
        tip_collection_account: AnchorPubkey,
        bump: u8,
    ) -> anchor_lang::solana_program::instruction::Instruction {
        initialize_revenue_share_account_v1_ix(
            reward_distribution_program_id,
            InitializeRevenueShareAccountV1Args {
                record_authority,
                share_kind: reward_distribution::state::RevenueKind::Tip,
                name,
                bump,
            },
            InitializeRevenueShareAccountV1Accounts {
                tips_and_mev_share_config: derive_tips_and_mev_share_config_address(
                    &reward_distribution_program_id,
                )
                .0,
                system_program: AnchorPubkey::from(system_program::id().as_array().clone()),
                revenue_share_account: tip_collection_account,
                rakurai_activation_account: derive_activation_account_address(
                    &AnchorPubkey::from(
                        self.distribution_config
                            .rakurai_activation_program_id
                            .as_array()
                            .clone(),
                    ),
                    &identity,
                )
                .0,
                validator_vote_account: vote_account,
                payer: identity,
            },
        )
    }

    fn start_tip_turn_tracking(&mut self, bank: &Bank, leader_slot: Slot) {
        let first_slot = first_of_consecutive_leader_slots(leader_slot);
        let last_slot = last_of_consecutive_leader_slots(leader_slot);

        if first_slot == 0 {
            warn!("reward_distributor tip_turn_tracking skipped: first_slot is 0");
            return;
        }

        // The parent is the last frozen ancestor bank, i.e. the canonical state entering the
        // turn. We snapshot start balances here because the bank will be pruned from
        // bank_forks long before the turn slots are rooted.
        let Some(baseline_bank) = bank.parent() else {
            warn!(
                "reward_distributor tip_turn_tracking skipped: no parent bank for first_slot={first_slot}"
            );
            return;
        };
        let baseline_slot = baseline_bank.slot();

        match load_cached_uuid_tip_groups(bank) {
            Some(groups) if !groups.is_empty() => {
                let groups: Vec<CachedUuidTipGroup> = groups
                    .into_iter()
                    .filter(|group| group.uuid_name != RAKURAI_REVENUE_NAME)
                    .collect();
                if groups.is_empty() {
                    return;
                }
                let start_balances = snapshot_group_balances(&baseline_bank, &groups);
                let uuid_group_entries = groups
                    .iter()
                    .map(|group| format!("{}:{}", group.uuid, group.entries.len()))
                    .collect::<Vec<_>>()
                    .join(", ");
                info!(
                    "reward_distributor tip_turn_tracking started first_slot={first_slot} \
                     last_slot={last_slot} baseline_slot={baseline_slot} uuid_groups={} \
                     uuid_entries=[{uuid_group_entries}]",
                    groups.len(),
                );
                self.active_tip_turn = Some(TipTurnReport {
                    first_slot,
                    last_slot,
                    baseline_slot,
                    groups,
                    start_balances,
                    end_balances: None,
                    end_captured_slot: None,
                });
            }
            Some(_) => {
                warn!("reward_distributor tip_turn_tracking: empty uuid groups");
            }
            None => {
                warn!(
                    "reward_distributor tip_turn_tracking: failed to load virtual priority config"
                );
            }
        }
    }

    fn enqueue_tip_turn_report(&mut self) {
        if let Some(report) = self.active_tip_turn.take() {
            info!(
                "reward_distributor tip_turn_tracking enqueued first_slot={} last_slot={}",
                report.first_slot, report.last_slot
            );
            self.pending_tip_reports.push(report);
        }
    }

    fn try_finalize_pending_tip_reports(&mut self) {
        if self.pending_tip_reports.is_empty() {
            return;
        }

        let (root_slot, highest_slot) = match self.bank_forks.read().ok() {
            Some(bank_forks_r) => (bank_forks_r.root(), bank_forks_r.working_bank().slot()),
            None => return,
        };

        let reports = std::mem::take(&mut self.pending_tip_reports);
        for mut report in reports {
            // Step 1: capture end-of-turn balances while the leader banks are still retained
            // in bank_forks. They get pruned once the root advances past them, so we cannot
            // defer this until the slots are rooted.
            //
            // We must snapshot from the last slot of the turn to account for tips accrued in
            // every slot. The report is enqueued on the Forward decision, but the last leader
            // bank may still be freezing at that instant, so we prefer the exact `last_slot`
            // bank and only fall back to the highest frozen bank in the window once the turn
            // is definitively over (a bank beyond the window exists, or the root has passed
            // it). That fallback covers the case where the last slot(s) were skipped.
            if report.end_balances.is_none() {
                let turn_over = highest_slot > report.last_slot || root_slot > report.last_slot;
                if let Some(end_bank) = self.frozen_bank_at(report.last_slot) {
                    report.end_balances = Some(snapshot_group_balances(&end_bank, &report.groups));
                    report.end_captured_slot = Some(report.last_slot);
                } else if turn_over {
                    if let Some((end_slot, end_bank)) =
                        self.highest_frozen_bank_in_window(report.first_slot, report.last_slot)
                    {
                        report.end_balances =
                            Some(snapshot_group_balances(&end_bank, &report.groups));
                        report.end_captured_slot = Some(end_slot);
                    } else {
                        // The turn window is fully behind us but no leader bank was ever
                        // observed frozen (e.g. the whole turn was skipped). Drop it.
                        warn!(
                            "reward_distributor tip_turn_complete dropped: no frozen leader bank \
                             for first_slot={} last_slot={} (root={root_slot}, highest={highest_slot})",
                            report.first_slot, report.last_slot
                        );
                        continue;
                    }
                } else {
                    // Last leader bank not frozen yet and the turn is not definitively over;
                    // wait for a later poll so we don't undercount trailing slots.
                    self.pending_tip_reports.push(report);
                    continue;
                }
            }

            let end_captured_slot = report
                .end_captured_slot
                .expect("end_captured_slot set when end_balances is Some");

            // Step 2: only finalize once the captured slot is rooted, so the snapshotted
            // balances are canonical. Ancestors of a rooted bank (including the baseline)
            // are guaranteed rooted as well.
            if !self.blockstore.is_root(end_captured_slot) {
                if root_slot > end_captured_slot {
                    // Root advanced past the captured slot without rooting it: the bank we
                    // snapshotted was on an abandoned fork. Drop the report.
                    warn!(
                        "reward_distributor tip_turn_complete dropped: captured slot \
                         {end_captured_slot} not rooted (root={root_slot}, first_slot={}, \
                         last_slot={})",
                        report.first_slot, report.last_slot
                    );
                    continue;
                }
                self.pending_tip_reports.push(report);
                continue;
            }

            // Step 3: compute weighted deltas from the captured snapshots and queue them.
            let end_balances = report
                .end_balances
                .as_ref()
                .expect("end_balances set at this point");
            let deltas = weighted_tip_deltas_from_balances(
                &report.start_balances,
                end_balances,
                &report.groups,
            );
            info!(
                "reward_distributor tip_turn_complete first_slot={} last_slot={} \
                 baseline_slot={} end_slot={end_captured_slot}",
                report.first_slot, report.last_slot, report.baseline_slot
            );
            self.queue_tip_revenue_updates_from_deltas(deltas, report.first_slot, report.last_slot);
        }
    }

    /// If `op` already has an in-flight txn, retry it until it lands.
    /// Returns `true` when the caller should skip building a new txn for this op
    /// (already landed, waiting on retry interval, or retrying the same signed txn).
    fn retry_pending_rakurai_op(
        &self,
        op: &mut RakuraiOpTxn,
        bank: &Bank,
        kind: &str,
        retry_interval: Duration,
    ) -> bool {
        if op.landed {
            return true;
        }
        let Some(runtime_tx) = op.txn.as_ref() else {
            return false;
        };
        if op.last_send.elapsed() <= retry_interval {
            return true;
        }
        if bank
            .get_signature_status_with_blockhash(
                runtime_tx.as_sanitized_transaction().signature(),
                runtime_tx.as_sanitized_transaction().recent_blockhash(),
            )
            .is_none()
        {
            self.send_transaction(format!("{kind}_retry=true"), bank, runtime_tx.clone());
            op.last_send = Instant::now();
        } else {
            op.landed();
        }
        true
    }

    /// Build, track, and send a single-task rakurai op transaction.
    /// Returns `true` if a runtime transaction was created and handed to the scheduler.
    fn dispatch_rakurai_op(
        &mut self,
        op: &mut RakuraiOpTxn,
        bank: &Bank,
        instructions: &[Instruction],
        kind: &str,
        extra: String,
        track_rewards: bool,
    ) -> bool {
        if instructions.is_empty() {
            return false;
        }
        let Some(runtime_tx) = self.create_runtime_transaction(bank, instructions) else {
            return false;
        };
        if track_rewards {
            self.txns_history.insert(
                runtime_tx.signature().clone(),
                TxnsHistory {
                    rewards: self.accumulated_reward,
                    message_hash: runtime_tx.message_hash().clone(),
                    send_slot: bank.slot(),
                    blockhash: runtime_tx.recent_blockhash().clone(),
                },
            );
            self.accumulated_reward = 0;
        }
        info!(
            "reward_distributor txn_sent kind={kind} instructions={} sig={}",
            instructions.len(),
            runtime_tx.signature()
        );
        op.txn(runtime_tx.clone());
        let txn_kind = if extra.is_empty() {
            kind.to_string()
        } else {
            format!("{kind},{extra}")
        };
        self.send_transaction(txn_kind, bank, runtime_tx);
        true
    }

    pub fn run(mut self) -> Result<(), SchedulerError> {
        let mut decision;
        let mut slot_rewards: HashMap<Slot, u64> = HashMap::new();
        let mut buffered_slots = Vec::new();
        let mut prev_decision = BufferedPacketsDecision::Hold;
        let mut switching_point;
        let mut slot;
        let mut previous_slot = 0;
        #[cfg(feature = "build_validator")]
        let mut rakurai_enabled_flag;
        // leader's last slot, used to detect the change in slot
        let mut last_leader_slot: u64 = 0;
        // used to detect the turn has started, it will be set at first consume decision and reset at first forward decision
        let mut turn_started: bool = false;

        // this is const of 5ms because make_consume_or_forward_decision updates its decision after every 5 ms
        // so polling make_consume_or_forward_decision at a higher frequency is not needed
        let timeout_ms = Duration::from_millis(5);
        let mut last_epoch = 0;

        let mut rakurai_ops = RakuraiOpTxnSet::new();
        let mut is_tip_receiver_changed = false;
        // Retry interval while an op txn is in-flight and has not yet landed
        let rakurai_op_txn_retry_interval_ms = Duration::from_millis(25);
        let mut last_log_slot = 0;

        let mut tip_turn_to_start: Option<(Arc<Bank>, Slot)> = None;
        // Set at the first slot of a leader turn to scan on-chain TCAs and run conversions.
        let mut conversions_to_process: Option<Arc<Bank>> = None;

        loop {
            std::thread::sleep(timeout_ms);

            #[cfg(feature = "build_validator")]
            if self.reset_rakurai.load(Relaxed) {
                unsafe {
                    info!("resetting rakurai");
                    reset_rakurai();
                }

                self.reset_rakurai.store(false, Relaxed);
            }
            // Get the current decision and switching point flag from the decision maker
            (decision, switching_point, slot) =
                self.decision_maker.make_consume_or_forward_decision();

            if decision != prev_decision {
                match &decision {
                    BufferedPacketsDecision::Consume(bank_start) => {
                        let root_bank = self.bank_forks.read().unwrap().root_bank();
                        if let Ok(mut shared_bank_update) = self.shared_bank_update.write() {
                            *shared_bank_update = LatestBankPair::new(
                                root_bank.clone(),
                                bank_start.working_bank.clone(),
                            );
                        }
                        previous_slot = bank_start.working_bank.slot();
                    }
                    _ => {
                        let root_bank = self.bank_forks.read().unwrap().root_bank();
                        let working_bank = self.bank_forks.read().unwrap().working_bank();
                        if let Ok(mut shared_bank_update) = self.shared_bank_update.write() {
                            *shared_bank_update =
                                LatestBankPair::new(root_bank.clone(), working_bank.clone());
                        }
                        previous_slot = working_bank.slot();
                    }
                }
            } else if Self::is_slot_changed(&slot, &mut previous_slot) {
                if let Ok(bank_forks_read_lock) = self.bank_forks.read() {
                    let root_bank = bank_forks_read_lock.root_bank();
                    let working_bank = bank_forks_read_lock.working_bank();
                    if let Ok(mut shared_bank_update) = self.shared_bank_update.write() {
                        *shared_bank_update =
                            LatestBankPair::new(root_bank.clone(), working_bank.clone());
                    }
                    previous_slot = working_bank.slot();
                }
            }

            // Update the switching point flag when in forwarding because it only changes during forwarding decision
            if let BufferedPacketsDecision::Forward = decision {
                self.shared_decision.1.store(switching_point, Relaxed);
            }

            // Only update decision state if it has changed
            if decision != prev_decision {
                prev_decision = decision.clone();

                let mut decision_state_lock = self.shared_decision.0.write().unwrap();
                *decision_state_lock = translate_decision_into_decision_state(&decision);

                if let BufferedPacketsDecision::Consume(bank_start) = &decision {
                    let new_leader_slot = bank_start.working_bank.slot();
                    if new_leader_slot != last_leader_slot {
                        last_leader_slot = new_leader_slot;
                        #[cfg(feature = "build_validator")]
                        unsafe {
                            rakurai_enabled_flag = rakurai_enabled();
                        }
                        #[cfg(feature = "build_validator")]
                        {
                            if rakurai_enabled_flag {
                                // Push new slots if Rakurai is enabled
                                buffered_slots.push(new_leader_slot);
                                if leader_slot_index(new_leader_slot) == 0 {
                                    tip_turn_to_start =
                                        Some((bank_start.working_bank.clone(), new_leader_slot));
                                    // Scan on-chain TCAs and run block-reward conversions once per turn.
                                    conversions_to_process = Some(bank_start.working_bank.clone());
                                }
                            }
                        }

                        if !turn_started {
                            // Mark the turn as started
                            turn_started = true;

                            // Check for epoch change
                            let current_epoch = bank_start.working_bank.epoch();
                            if last_epoch != 0 && last_epoch != current_epoch {
                                info!(
                                    "reward_distributor epoch changed from {} to {}, slot {}",
                                    last_epoch, current_epoch, slot
                                );
                                // Reset MEV commission status on epoch change
                                self.rakurai_commission_on_mev_commission_stats =
                                    RakuraiCommissionOnMevStatus::NotDeducted;
                            }
                            last_epoch = current_epoch;

                            // Reset MEV commission status for new turn (except if already deducted or skipped)
                            if self.rakurai_commission_on_mev_commission_stats
                                == RakuraiCommissionOnMevStatus::TransactionSent
                            {
                                self.rakurai_commission_on_mev_commission_stats =
                                    RakuraiCommissionOnMevStatus::NotDeducted;
                            }
                        }
                    }
                }
            }

            if let Some((working_bank, leader_slot)) = tip_turn_to_start.take() {
                self.start_tip_turn_tracking(&working_bank, leader_slot);
            }

            if let Some(working_bank) = conversions_to_process.take() {
                self.process_block_reward_conversions_from_chain(&working_bank);
                self.process_block_reward_conversions_from_chain_v1(&working_bank);
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
                    let (rca_created, reward_collection_account) =
                        self.get_reward_collection_pda_status(&working_bank);

                    // create-rca
                    if !self.retry_pending_rakurai_op(
                        &mut rakurai_ops.create_rca,
                        &working_bank,
                        "create-rca",
                        rakurai_op_txn_retry_interval_ms,
                    ) && !rca_created
                    {
                        match self.initialize_reward_collection_account_instruction(&working_bank) {
                            Ok(ix) => {
                                debug!(
                                    "reward_distributor initialize_reward_collection_account_instruction"
                                );
                                self.dispatch_rakurai_op(
                                    &mut rakurai_ops.create_rca,
                                    &working_bank,
                                    &[ix],
                                    "create-rca",
                                    format!("rca={reward_collection_account}"),
                                    false,
                                );
                            }
                            Err(e) => {
                                error!(
                                    "reward_distributor error in initialize_reward_collection_account_instruction: {e}"
                                );
                            }
                        }
                    }

                    // change-tip-receiver
                    if !self.retry_pending_rakurai_op(
                        &mut rakurai_ops.change_tip_receiver,
                        &working_bank,
                        "change-tip-receiver",
                        rakurai_op_txn_retry_interval_ms,
                    ) {
                        is_tip_receiver_changed = false;
                        match self.change_tip_receiver_instruction(
                            &working_bank,
                            &mut is_tip_receiver_changed,
                        ) {
                            Ok(maybe_ix) => match maybe_ix {
                                Some(ix) if !ix.is_empty() => {
                                    debug!("reward_distributor change_tip_receiver_instruction");
                                    self.dispatch_rakurai_op(
                                        &mut rakurai_ops.change_tip_receiver,
                                        &working_bank,
                                        &ix,
                                        "change-tip-receiver",
                                        format!(
                                            "is_tip_receiver_changed={is_tip_receiver_changed}"
                                        ),
                                        false,
                                    );
                                }
                                _ => {
                                    debug!(
                                        "reward_distributor change_tip_receiver_instruction not required"
                                    );
                                }
                            },
                            Err(e) => {
                                error!(
                                    "reward_distributor error in change_tip_receiver_instruction: {e}"
                                );
                            }
                        }
                    }

                    // Record tip revenue deltas from the previous leader turn on-chain.
                    if !self.retry_pending_rakurai_op(
                        &mut rakurai_ops.record_ix,
                        &working_bank,
                        "record-ix",
                        rakurai_op_txn_retry_interval_ms,
                    ) {
                        let mut record_instructions = Vec::new();
                        match self.append_pending_tip_revenue_instructions(
                            &working_bank,
                            &mut record_instructions,
                        ) {
                            Ok(mut queued) => {
                                if self.dispatch_rakurai_op(
                                    &mut rakurai_ops.record_ix,
                                    &working_bank,
                                    &record_instructions,
                                    "record-ix",
                                    String::new(),
                                    false,
                                ) {
                                    self.pending_tip_revenue_in_flight = queued;
                                } else {
                                    self.pending_tip_revenue_updates.append(&mut queued);
                                }
                            }
                            Err(e) => {
                                error!(
                                    "reward_distributor error in append_pending_tip_revenue_instructions: {e}"
                                );
                            }
                        }
                    } else if rakurai_ops.record_ix.landed {
                        self.pending_tip_revenue_in_flight.clear();
                    }

                    // transfer-staker-rewards (requires this epoch's RCA)
                    if !self.retry_pending_rakurai_op(
                        &mut rakurai_ops.transfer_staker_rewards,
                        &working_bank,
                        "transfer-staker-rewards",
                        rakurai_op_txn_retry_interval_ms,
                    ) {
                        let rca_ready = rca_created || rakurai_ops.create_rca.landed;
                        if rca_ready && self.accumulated_reward > 0 {
                            match self.create_transfer_rca_instruction(
                                self.accumulated_reward,
                                reward_collection_account,
                                &working_bank,
                            ) {
                                Ok(maybe_ix) => match maybe_ix {
                                    Some(ix) => {
                                        debug!(
                                            "reward_distributor create_transfer_rca_instruction"
                                        );
                                        self.dispatch_rakurai_op(
                                            &mut rakurai_ops.transfer_staker_rewards,
                                            &working_bank,
                                            &[ix],
                                            "transfer-staker-rewards",
                                            format!("rca={reward_collection_account}"),
                                            true,
                                        );
                                    }
                                    None => {
                                        debug!(
                                            "reward_distributor create_transfer_rca_instruction not required"
                                        );
                                    }
                                },
                                Err(e) => {
                                    error!(
                                        "reward_distributor error in create_transfer_rca_instruction: {e}"
                                    );
                                }
                            }
                        }
                    }

                    // mev_commission
                    if !self.retry_pending_rakurai_op(
                        &mut rakurai_ops.mev_commission,
                        &working_bank,
                        "mev-commission",
                        rakurai_op_txn_retry_interval_ms,
                    ) {
                        match self.rakurai_commission_on_mev_commission_stats {
                            RakuraiCommissionOnMevStatus::NotDeducted => {
                                let (should_deduct, rca_pda, tda_pda) =
                                    self.should_deduct_mev_commission(&working_bank, 50);
                                if should_deduct && rca_pda.is_some() && tda_pda.is_some() {
                                    info!("reward_distributor deducting mev commission");
                                    match self.transfer_mev_commission(
                                        &working_bank,
                                        rca_pda.unwrap(),
                                        tda_pda.unwrap(),
                                    ) {
                                        Ok(maybe_ix) => match maybe_ix {
                                            Some(ix) => {
                                                debug!(
                                                    "reward_distributor transfer_mev_commission"
                                                );
                                                self.dispatch_rakurai_op(
                                                    &mut rakurai_ops.mev_commission,
                                                    &working_bank,
                                                    &[ix],
                                                    "mev-commission",
                                                    String::new(),
                                                    false,
                                                );
                                            }
                                            None => {
                                                debug!(
                                                    "reward_distributor transfer_mev_commission not required"
                                                );
                                            }
                                        },
                                        Err(e) => {
                                            error!("error in transfer_mev_commission: {e}");
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    // Log once per leader slot
                    if working_bank.slot() != last_log_slot {
                        last_log_slot = working_bank.slot();
                        info!(
                            "reward_distributor decision=consume, rca={}, epoch={}, slot={}, rca_status={:?}, rakurai_commission_on_mev_commission_stats={:?}, change_tip_receiver_instruction={}, create_rca_landed={}, change_tip_receiver_landed={}, record_ix_landed={}, transfer_staker_rewards_landed={}, mev_commission_landed={}, pending_txns_count={}, pending_txns={:?}",
                            reward_collection_account,
                            working_bank.epoch(),
                            working_bank.slot(),
                            rca_created,
                            self.rakurai_commission_on_mev_commission_stats,
                            is_tip_receiver_changed,
                            rakurai_ops.create_rca.landed,
                            rakurai_ops.change_tip_receiver.landed,
                            rakurai_ops.record_ix.landed,
                            rakurai_ops.transfer_staker_rewards.landed,
                            rakurai_ops.mev_commission.landed,
                            self.txns_history.len(),
                            self.txns_history
                                .iter()
                                .map(|(sig, tx)| (sig, tx.send_slot, tx.rewards))
                                .collect::<Vec<_>>()
                        );
                    }
                }

                BufferedPacketsDecision::Forward => {
                    self.read_rewards_and_check_txn_history(&mut slot_rewards, &mut buffered_slots);
                    self.enqueue_tip_turn_report();
                    self.try_finalize_pending_tip_reports();
                    if !rakurai_ops.record_ix.landed {
                        self.restore_unlanded_tip_revenue_updates();
                    }
                    turn_started = false;
                    rakurai_ops.reset();
                }

                BufferedPacketsDecision::Hold => {}
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum RewardDistributorError {
    #[error("RCA account not found")]
    RcaAccountNotFound,

    #[error("RCA deserialization failed")]
    RcaDeserializationFailed,

    #[error("RAA config account not found")]
    RaaConfigAccountNotFound,

    #[error("RAA config deserialization failed")]
    RaaConfigDeserializationFailed,

    #[error("RAA account not found")]
    RaaAccountNotFound,

    #[error("RAA deserialization failed")]
    RaaDeserializationFailed,

    #[error("Tip config account not found")]
    TipConfigAccountNotFound,

    #[error("Tip config deserialization failed")]
    TipConfigDeserializationFailed,
}
