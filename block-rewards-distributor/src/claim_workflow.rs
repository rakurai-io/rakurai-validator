use {
    crate::{send_until_blockhash_expires, GeneratedMerkleTreeCollection},
    anchor_lang::{prelude::Pubkey as AnchorPubkey, AccountDeserialize},
    itertools::Itertools,
    log::{error, info, warn},
    reward_distribution::{
        sdk::instruction::{claim_ix, ClaimAccounts, ClaimArgs},
        state::{ClaimStatus, RewardCollectionAccount, RewardDistributionConfigAccount},
    },
    serde::{Deserialize, Serialize},
    solana_client::{nonblocking::rpc_client::RpcClient, rpc_client::SerializableTransaction},
    solana_commitment_config::CommitmentConfig,
    solana_compute_budget_interface::ComputeBudgetInstruction,
    solana_metrics::datapoint_info,
    solana_program::{
        fee_calculator::DEFAULT_TARGET_LAMPORTS_PER_SIGNATURE, native_token::LAMPORTS_PER_SOL,
    },
    solana_rpc_client_api::config::RpcSimulateTransactionConfig,
    solana_sdk::{
        account::Account,
        instruction::{AccountMeta, Instruction},
        pubkey::Pubkey,
        signature::{Keypair, Signature, Signer},
        transaction::Transaction,
    },
    std::{
        collections::HashMap,
        sync::Arc,
        thread,
        time::{Duration, Instant},
    },
    thiserror::Error,
};
#[derive(Default, Clone, Deserialize, Serialize, Debug)]
pub struct RewardsInfo {
    pub transaction: Transaction,
    pub active_stake: u64,
    pub validator_vote_account: Pubkey,
    pub validator_node_pubkey: Pubkey,
    pub stake_account_pubkey: Pubkey,
    pub claimant: Pubkey,
    pub staker_pubkey: Pubkey,
    pub status: Option<String>,
    pub epoch: u64,
    pub amount: u64,
    pub priority: u8,
}

#[derive(Error, Debug)]
pub enum ClaimRewardsError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    JsonError(#[from] serde_json::Error),

    #[error(transparent)]
    AnchorError(anchor_lang::error::Error),

    #[error(transparent)]
    RpcError(#[from] solana_rpc_client_api::client_error::Error),

    #[error("Expected to have at least {desired_balance} lamports in {payer:?}. Current balance is {start_balance} lamports. Deposit {sol_to_deposit} SOL to continue.")]
    InsufficientBalance {
        desired_balance: u64,
        payer: Pubkey,
        start_balance: u64,
        sol_to_deposit: u64,
    },

    #[error("Not finished with job, transactions left {transactions_left}")]
    NotFinished { transactions_left: usize },

    #[error("UncaughtError {e:?}")]
    UncaughtError { e: String },
}

pub async fn get_claim_transactions_for_valid_unclaimed(
    rpc_client: &RpcClient,
    merkle_trees: &GeneratedMerkleTreeCollection,
    reward_distribution_program_id: Pubkey,
    micro_lamports: u64,
    payer_pubkey: Pubkey,
) -> Result<Vec<RewardsInfo>, ClaimRewardsError> {
    let tree_nodes = merkle_trees
        .generated_merkle_trees
        .iter()
        .flat_map(|tree| &tree.tree_nodes)
        .collect_vec();

    info!(
        "reading rewards distribution related accounts for epoch {}",
        tree_nodes.len()
    );

    let rda_pubkeys = merkle_trees
        .generated_merkle_trees
        .iter()
        .map(|tree| tree.reward_distribution_account)
        .collect_vec();
    let rdas: HashMap<Pubkey, Account> = crate::get_batched_accounts(rpc_client, &rda_pubkeys)
        .await?
        .into_iter()
        .filter_map(|(pubkey, a)| Some((pubkey, a?)))
        .collect();

    let claimant_pubkeys = tree_nodes
        .iter()
        .map(|tree_node| tree_node.claimant)
        .collect_vec();
    let claimants: HashMap<Pubkey, Account> =
        crate::get_batched_accounts(rpc_client, &claimant_pubkeys)
            .await?
            .into_iter()
            .filter_map(|(pubkey, a)| Some((pubkey, a?)))
            .collect();

    let claim_status_pubkeys = tree_nodes
        .iter()
        .map(|tree_node| tree_node.claim_status_pubkey)
        .collect_vec();
    let claim_statuses: HashMap<Pubkey, Account> =
        crate::get_batched_accounts(rpc_client, &claim_status_pubkeys)
            .await?
            .into_iter()
            .filter_map(|(pubkey, a)| Some((pubkey, a?)))
            .collect();

    let transactions = build_reward_claim_transactions(
        reward_distribution_program_id,
        merkle_trees,
        rdas,
        claimants,
        claim_statuses,
        micro_lamports,
        payer_pubkey,
    );

    Ok(transactions)
}

pub async fn claim_rewards(
    merkle_trees: &GeneratedMerkleTreeCollection,
    rpc_url: String,
    reward_distribution_program_id: Pubkey,
    keypair: Arc<Keypair>,
    max_loop_duration: Duration,
    micro_lamports: u64,
) -> Result<(), ClaimRewardsError> {
    let rpc_client = RpcClient::new_with_timeout_and_commitment(
        rpc_url,
        Duration::from_secs(300),
        CommitmentConfig::confirmed(),
    );

    let start = Instant::now();
    while start.elapsed() <= max_loop_duration {
        let all_claim_transactions = get_claim_transactions_for_valid_unclaimed(
            &rpc_client,
            merkle_trees,
            reward_distribution_program_id,
            micro_lamports,
            keypair.pubkey(),
        )
        .await?;

        if all_claim_transactions.is_empty() {
            return Ok(());
        }

        let mut transactions_with_priority: Vec<(Transaction, u8)> = all_claim_transactions
            .iter()
            .map(|info| (info.transaction.clone(), info.priority))
            .collect();

        transactions_with_priority.sort_by(|a, b| b.1.cmp(&a.1));

        let transactions: Vec<_> = transactions_with_priority
            .into_iter()
            .take(10_000)
            .collect();

        // only check balance for the ones we need to currently send since reclaim rent running in parallel
        if let Some((start_balance, desired_balance, sol_to_deposit)) =
            is_sufficient_balance(&keypair.pubkey(), &rpc_client, transactions.len() as u64).await
        {
            return Err(ClaimRewardsError::InsufficientBalance {
                desired_balance,
                payer: keypair.pubkey(),
                start_balance,
                sol_to_deposit,
            });
        }

        let blockhash = rpc_client.get_latest_blockhash().await?;

        let mut txns_to_claim: HashMap<Signature, RewardsInfo> = all_claim_transactions
            .into_iter()
            .map(|mut reward_info| {
                reward_info.transaction.sign(&[&keypair], blockhash);

                let signature = *reward_info.transaction.get_signature();
                reward_info.status = Some(signature.to_string());

                (signature, reward_info)
            })
            .collect();

        let mut sorted_txns: Vec<(Signature, RewardsInfo)> =
            txns_to_claim.clone().into_iter().collect();

        sorted_txns.sort_by(|a, b| b.1.priority.cmp(&a.1.priority));

        let claim_transactions: Vec<(Signature, Transaction)> = sorted_txns
            .into_iter()
            .map(|(signature, reward_info)| (signature, reward_info.transaction))
            .collect();

        match send_until_blockhash_expires(&rpc_client, claim_transactions, blockhash).await {
            Ok(((), processed_signatures)) => {
                for (signature, slot) in processed_signatures {
                    if let Some(rewards_info) = txns_to_claim.get_mut(&signature) {
                        datapoint_info!(
                            "rewards_info",
                            ("epoch", rewards_info.epoch, i64),
                            (
                                "vote_account",
                                &rewards_info.validator_vote_account.to_string(),
                                String
                            ),
                            ("active_stake", rewards_info.active_stake as i64, i64),
                            (
                                "identity",
                                &rewards_info.validator_node_pubkey.to_string(),
                                String
                            ),
                            (
                                "stake_pubkey",
                                &rewards_info.stake_account_pubkey.to_string(),
                                String
                            ),
                            ("staker", &rewards_info.staker_pubkey.to_string(), String),
                            ("claimant", &rewards_info.claimant.to_string(), String),
                            ("rewards", rewards_info.amount, i64),
                            (
                                "status",
                                rewards_info.status.as_deref().unwrap_or("None"),
                                String
                            ),
                            ("slot", slot, i64),
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("Error: {:?}", e);
            }
        }
    }

    //to ensure that that the data should be written to db before application stop.
    thread::sleep(Duration::from_secs(12));

    let transactions = get_claim_transactions_for_valid_unclaimed(
        &rpc_client,
        merkle_trees,
        reward_distribution_program_id,
        micro_lamports,
        keypair.pubkey(),
    )
    .await?;
    if transactions.is_empty() {
        return Ok(());
    }

    // if more transactions left, we'll simulate them all to make sure its not an uncaught error
    let mut is_error = false;
    let mut error_str = String::new();
    for rewards_info in &transactions {
        match rpc_client
            .simulate_transaction_with_config(
                &rewards_info.transaction,
                RpcSimulateTransactionConfig {
                    sig_verify: false,
                    replace_recent_blockhash: true,
                    commitment: Some(CommitmentConfig::processed()),
                    ..RpcSimulateTransactionConfig::default()
                },
            )
            .await
        {
            Ok(_) => {}
            Err(e) => {
                error_str = e.to_string();
                is_error = true;

                match e.get_transaction_error() {
                    None => {
                        break;
                    }
                    Some(e) => {
                        warn!(
                            "transaction error. tx: {:?} error: {:?}",
                            &rewards_info.transaction, e
                        );
                        break;
                    }
                }
            }
        }
    }

    if is_error {
        Err(ClaimRewardsError::UncaughtError { e: error_str })
    } else {
        Err(ClaimRewardsError::NotFinished {
            transactions_left: transactions.len(),
        })
    }
}

/// Returns a list of claim transactions for valid, unclaimed rewards
/// A valid, unclaimed transaction consists of the following:
/// - there must be lamports to claim for the reward distribution account.
/// - there must be a merkle root.
/// - the claimant (typically a stake account) must exist.
/// - the claimant (typically a stake account) must have a non-zero amount of rewards to claim
/// - the claimant must have enough lamports post-claim to be rent-exempt.
///   - note: there aren't any rent exempt accounts on solana mainnet anymore.
/// - it must not have already been claimed.
fn build_reward_claim_transactions(
    reward_distribution_program_id: Pubkey,
    merkle_trees: &GeneratedMerkleTreeCollection,
    rdas: HashMap<Pubkey, Account>,
    claimants: HashMap<Pubkey, Account>,
    claim_status: HashMap<Pubkey, Account>,
    micro_lamports: u64,
    payer_pubkey: Pubkey,
) -> Vec<RewardsInfo> {
    let reward_distribution_accounts: HashMap<Pubkey, RewardCollectionAccount> = rdas
        .iter()
        .filter_map(|(pubkey, account)| {
            Some((
                *pubkey,
                RewardCollectionAccount::try_deserialize(&mut account.data.as_slice()).ok()?,
            ))
        })
        .collect();

    let claim_statuses: HashMap<Pubkey, ClaimStatus> = claim_status
        .iter()
        .filter_map(|(pubkey, account)| {
            Some((
                *pubkey,
                ClaimStatus::try_deserialize(&mut account.data.as_slice()).ok()?,
            ))
        })
        .collect();

    let reward_distribution_config = Pubkey::find_program_address(
        &[RewardDistributionConfigAccount::SEED],
        &reward_distribution_program_id,
    )
    .0;

    let mut rewards_info: Vec<RewardsInfo> = Vec::new();

    for tree in &merkle_trees.generated_merkle_trees {
        if tree.max_total_claim == 0 {
            continue;
        }

        let reward_distribution_account = reward_distribution_accounts
            .get(&tree.reward_distribution_account)
            .unwrap();

        if reward_distribution_account.merkle_root.is_none() {
            continue;
        }

        for node in &tree.tree_nodes {
            if !claimants.contains_key(&node.claimant)
                || claim_statuses.contains_key(&node.claim_status_pubkey)
                || node.amount == 0
            {
                continue;
            }

            let mut ix = claim_ix(
                AnchorPubkey::from(reward_distribution_program_id.as_array().clone()),
                ClaimArgs {
                    proof: node.proof.clone().unwrap(),
                    amount: node.amount,
                    bump: node.claim_status_bump,
                },
                ClaimAccounts {
                    config: AnchorPubkey::from(reward_distribution_config.as_array().clone()),
                    reward_collection_account: AnchorPubkey::from(
                        tree.reward_distribution_account.as_array().clone(),
                    ),
                    claim_status: AnchorPubkey::from(node.claim_status_pubkey.as_array().clone()),
                    claimant: AnchorPubkey::from(node.claimant.as_array().clone()),
                    payer: AnchorPubkey::from(payer_pubkey.as_array().clone()),
                    system_program: AnchorPubkey::new_from_array(
                        solana_system_interface::program::id().as_array().clone(),
                    ),
                },
            );
            let acct_metas: Vec<AccountMeta> = ix
                .accounts
                .iter_mut()
                .map(|acct| AccountMeta {
                    pubkey: Pubkey::from(acct.pubkey.as_array().clone()),
                    is_signer: acct.is_signer,
                    is_writable: acct.is_writable,
                })
                .collect();

            let instruction =
                Instruction::new_with_bytes(reward_distribution_program_id, &ix.data, acct_metas);

            let priority_fee_ix = ComputeBudgetInstruction::set_compute_unit_price(micro_lamports);

            let transaction =
                Transaction::new_with_payer(&[priority_fee_ix, instruction], Some(&payer_pubkey));

            rewards_info.push(RewardsInfo {
                transaction: transaction.clone(),
                active_stake: node.active_stake,
                validator_vote_account: tree.validator_vote_account,
                validator_node_pubkey: tree.validator_node_pubkey,
                stake_account_pubkey: node.stake_account_pubkey,
                claimant: node.claimant,
                staker_pubkey: node.staker_pubkey,
                status: None,
                epoch: merkle_trees.epoch,
                amount: node.amount,
                priority: node.priority,
            });
        }
    }

    rewards_info
}

/// heuristic to make sure we have enough funds to cover the rent costs if epoch has many validators
/// If insufficient funds, returns start balance, desired balance, and amount of sol to deposit
async fn is_sufficient_balance(
    payer: &Pubkey,
    rpc_client: &RpcClient,
    instruction_count: u64,
) -> Option<(u64, u64, u64)> {
    let start_balance = rpc_client
        .get_balance(payer)
        .await
        .expect("Failed to get starting balance");
    // most amounts are for 0 lamports. had 1736 non-zero claims out of 164742
    let min_rent_per_claim = rpc_client
        .get_minimum_balance_for_rent_exemption(ClaimStatus::SIZE)
        .await
        .expect("Failed to calculate min rent");
    let desired_balance = instruction_count
        .checked_mul(
            min_rent_per_claim
                .checked_add(DEFAULT_TARGET_LAMPORTS_PER_SIGNATURE)
                .unwrap(),
        )
        .unwrap();
    if start_balance < desired_balance {
        let sol_to_deposit = desired_balance
            .checked_sub(start_balance)
            .unwrap()
            .checked_add(LAMPORTS_PER_SOL)
            .unwrap()
            .checked_sub(1)
            .unwrap()
            .checked_div(LAMPORTS_PER_SOL)
            .unwrap(); // rounds up to nearest sol
        Some((start_balance, desired_balance, sol_to_deposit))
    } else {
        None
    }
}
