pub mod claim_workflow;
pub mod merkle_root_generator_workflow;
pub mod merkle_root_upload_workflow;
pub mod reclaim_rent_workflow;
pub mod stake_meta_generator_workflow;

use {
    crate::{
        merkle_root_generator_workflow::MerkleRootGeneratorError,
        reclaim_rent_workflow::CLOSE_INSTR_PER_TXN,
        stake_meta_generator_workflow::StakeMetaGeneratorError::CheckedMathError,
    },
    log::*,
    reward_distribution::state::{ClaimStatus, RewardCollectionAccount},
    serde::{de::DeserializeOwned, Deserialize, Serialize},
    solana_client::{
        nonblocking::rpc_client::RpcClient,
        rpc_client::{RpcClient as SyncRpcClient, SerializableTransaction},
    },
    solana_merkle_tree::MerkleTree,
    solana_metrics::{datapoint_error, datapoint_warn},
    solana_program::{borsh1::try_from_slice_unchecked, instruction::InstructionError},
    solana_rpc_client_api::{
        client_error::{Error, ErrorKind},
        config::RpcSendTransactionConfig,
        request::{RpcError, RpcResponseErrorData, MAX_MULTIPLE_ACCOUNTS},
        response::RpcSimulateTransactionResult,
    },
    solana_sdk::{
        account::{Account, AccountSharedData, ReadableAccount},
        clock::Slot,
        commitment_config::{CommitmentConfig, CommitmentLevel},
        hash::{Hash, Hasher},
        pubkey::Pubkey,
        signature::{Keypair, Signature},
        stake_history::Epoch,
        transaction::{
            Transaction,
            TransactionError::{self},
        },
    },
    solana_transaction_status::TransactionStatus,
    spl_stake_pool::{
        find_stake_program_address,
        state::{StakePool, ValidatorList},
    },
    std::{
        collections::{HashMap, HashSet},
        fs::{self, File},
        io::BufReader,
        num::NonZeroU32,
        path::{Path, PathBuf},
        str::FromStr,
        sync::Arc,
        time::{Duration, Instant},
    },
    tokio::{sync::Semaphore, time::sleep},
};

pub(crate) type ValidatorListError = Box<dyn std::error::Error>;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StakeAuthorityConfig {
    /// If provided, the rewards for these pool stake accounts
    /// are transferred to the pool's reserve account for faster activation.
    pub stake_pool_ids: Vec<String>,

    /// Per-validator filtering rules for stake authorities.
    pub validator_authority_filter: HashMap<String, ValidatorAuthorityRule>,
}

/// Rule for filtering stake authorities for a specific validator (vote account).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorAuthorityRule {
    /// List of stake authorities to include or exclude based on filter_mode.
    pub stake_authorities: Vec<String>,

    /// Determines how to interpret the stake_authorities list:
    /// - `include`: only the listed authorities are eligible for rewards.
    /// - `exclude`: the listed authorities are not eligible for rewards.
    pub filter_mode: FilterMode,
}

/// Indicates how to apply the stake authority filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterMode {
    Include,
    Exclude,
}

impl Default for FilterMode {
    fn default() -> Self {
        FilterMode::Exclude
    }
}

#[derive(Clone, Deserialize, Serialize, Debug)]
pub struct GeneratedMerkleTreeCollection {
    pub generated_merkle_trees: Vec<GeneratedMerkleTree>,
    pub bank_hash: String,
    pub epoch: Epoch,
    pub slot: Slot,
}

#[derive(Clone, Eq, Debug, Hash, PartialEq, Deserialize, Serialize)]
pub struct GeneratedMerkleTree {
    #[serde(with = "pubkey_string_conversion")]
    pub validator_vote_account: Pubkey,
    #[serde(with = "pubkey_string_conversion")]
    pub validator_node_pubkey: Pubkey,
    #[serde(with = "pubkey_string_conversion")]
    pub reward_distribution_account: Pubkey,
    #[serde(with = "pubkey_string_conversion")]
    pub merkle_root_upload_authority: Pubkey,
    pub merkle_root: Hash,
    pub tree_nodes: Vec<TreeNode>,
    pub total_delegated: u64,
    pub validator_commission_bps: u16,
    pub voting_commission: u8,
    pub max_total_claim: u64,
    pub max_num_nodes: u64,
    pub rakurai_commission_bps: u64,
}

pub fn read_stake_authority_config() -> StakeAuthorityConfig {
    let path_ref = Path::new("src/filtering_config.json");
    match fs::read_to_string(path_ref) {
        Ok(content) => match serde_json::from_str::<StakeAuthorityConfig>(&content) {
            Ok(config) => config,
            Err(err) => {
                warn!(
                    "Failed to parse JSON config at {}: {}. Proceeding with empty config.",
                    path_ref.display(),
                    err
                );
                StakeAuthorityConfig::default()
            }
        },
        Err(err) => {
            error!(
                "Failed to read config file {}: {}. Proceeding with default config.",
                path_ref.display(),
                err
            );
            StakeAuthorityConfig::default()
        }
    }
}

fn emit_inconsistent_tree_node_amount_dp(
    tree_nodes: &[TreeNode],
    reward_distribution_account: &Pubkey,
    rpc_client: &SyncRpcClient,
) {
    let actual_claims: u64 = tree_nodes.iter().map(|t| t.amount).sum();
    let rda = rpc_client.get_account(reward_distribution_account).unwrap();
    let min_rent = rpc_client
        .get_minimum_balance_for_rent_exemption(rda.data.len())
        .unwrap();

    let expected_claims = rda.lamports.checked_sub(min_rent).unwrap();
    if actual_claims == expected_claims {
        return;
    }

    if actual_claims > expected_claims {
        let diff = expected_claims as i64 - actual_claims as i64;
        datapoint_error!(
            "reward_distributor_error",
            (
                "actual_claims_exceeded",
                format!("reward_distribution_account={reward_distribution_account},actual_claims={actual_claims}, expected_claims={expected_claims}, offset={diff}"),
                String
            ),
        );
    } else {
        datapoint_warn!(
            "reward_distributor_warning",
            (
                "actual_claims_below",
                format!("reward_distribution_account={reward_distribution_account},actual_claims={actual_claims}, expected_claims={expected_claims}"),
                String
            ),
        );
    }
}

fn filter_stake_meta_by_authority_config(
    stake_meta: StakeMeta,
) -> (StakeMeta, Option<HashSet<String>>, u64) {
    let config = read_stake_authority_config();
    let validator_key = stake_meta.validator_vote_account.to_string();

    if let Some(rule) = config.validator_authority_filter.get(&validator_key) {
        let filtered_pubkeys: HashSet<String> = rule.stake_authorities.iter().cloned().collect();

        let total_filtered_lamports: u64 = match rule.filter_mode {
            FilterMode::Include => stake_meta
                .delegations
                .iter()
                .filter(|d| !filtered_pubkeys.contains(&d.staker_pubkey.to_string()))
                .map(|d| d.lamports_delegated)
                .sum(),
            FilterMode::Exclude => stake_meta
                .delegations
                .iter()
                .filter(|d| filtered_pubkeys.contains(&d.staker_pubkey.to_string()))
                .map(|d| d.lamports_delegated)
                .sum(),
        };

        return (stake_meta, Some(filtered_pubkeys), total_filtered_lamports);
    }
    (stake_meta, None, 0)
}

impl GeneratedMerkleTreeCollection {
    pub fn new_from_stake_meta_collection(
        stake_meta_coll: StakeMetaCollection,
        maybe_rpc_client: Option<SyncRpcClient>,
    ) -> Result<GeneratedMerkleTreeCollection, MerkleRootGeneratorError> {
        let pool_stake_account_of_interest = get_pool_stake_info(maybe_rpc_client.as_ref());
        let reward_distribution_program_id = stake_meta_coll.reward_distribution_program_id;
        let generated_merkle_trees = stake_meta_coll
            .stake_metas
            .into_iter()
            .filter(|stake_meta| {
                stake_meta
                    .maybe_reward_distribution_meta
                    .as_ref()
                    .map(|meta| meta.total_rewards > 0)
                    .unwrap_or(false)
            })
            .filter_map(|unfiltered_stake_meta| {
                let (stake_meta, maybe_excluded_stakers, _excluded_lamports) =
                    filter_stake_meta_by_authority_config(unfiltered_stake_meta);

                if stake_meta.delegations.is_empty() {
                    return None;
                }
                let validator_vote_account = stake_meta.validator_vote_account;
                let validator_node_pubkey = stake_meta.validator_node_pubkey;
                let voting_commission = stake_meta.commission;

                let amount_delegated = stake_meta.total_delegated;
                let mut tree_nodes = match TreeNode::vec_from_stake_meta(
                    &stake_meta,
                    pool_stake_account_of_interest.clone(),
                    maybe_excluded_stakers.unwrap_or_default(),
                    reward_distribution_program_id,
                ) {
                    Err(e) => return Some(Err(e)),
                    Ok(maybe_tree_nodes) => maybe_tree_nodes,
                }?;

                if let Some(rpc_client) = &maybe_rpc_client {
                    if let Some(rda) = stake_meta.maybe_reward_distribution_meta.as_ref() {
                        emit_inconsistent_tree_node_amount_dp(
                            &tree_nodes[..],
                            &rda.reward_distribution_pubkey,
                            rpc_client,
                        );
                    }
                }

                let hashed_nodes: Vec<[u8; 32]> =
                    tree_nodes.iter().map(|n| n.hash().to_bytes()).collect();

                let reward_distribution_meta = stake_meta.maybe_reward_distribution_meta.unwrap();

                let validator_commission_bps = reward_distribution_meta.validator_fee_bps;
                let merkle_tree = MerkleTree::new(&hashed_nodes[..], true);
                let max_num_nodes = tree_nodes.len() as u64;

                for (i, tree_node) in tree_nodes.iter_mut().enumerate() {
                    tree_node.proof = Some(get_proof(&merkle_tree, i));
                }

                Some(Ok(GeneratedMerkleTree {
                    validator_vote_account,
                    validator_node_pubkey,
                    max_num_nodes,
                    reward_distribution_account: reward_distribution_meta
                        .reward_distribution_pubkey,
                    merkle_root_upload_authority: reward_distribution_meta
                        .merkle_root_upload_authority,
                    merkle_root: *merkle_tree.get_root().unwrap(),
                    tree_nodes,
                    validator_commission_bps,
                    voting_commission,
                    total_delegated: amount_delegated,
                    max_total_claim: reward_distribution_meta.total_rewards,
                    rakurai_commission_bps: reward_distribution_meta.rakurai_commission_bps.into(),
                }))
            })
            .collect::<Result<Vec<GeneratedMerkleTree>, MerkleRootGeneratorError>>()?;

        Ok(GeneratedMerkleTreeCollection {
            generated_merkle_trees,
            bank_hash: stake_meta_coll.bank_hash,
            epoch: stake_meta_coll.epoch,
            slot: stake_meta_coll.slot,
        })
    }
}

pub fn get_pool_stake_info(maybe_rpc_client: Option<&SyncRpcClient>) -> HashMap<Pubkey, Pubkey> {
    let mut lst_pool: Vec<(Pubkey, Account)> = Vec::new();
    let config = read_stake_authority_config();
    if let Some(ref rpc_client) = maybe_rpc_client {
        for stake_pool_id in config.stake_pool_ids {
            let pubkey = Pubkey::from_str(&stake_pool_id).expect("Failed to parse STAKE_POOL_ID");
            match rpc_client.get_account(&pubkey) {
                Ok(account) => lst_pool.push((pubkey, account)),
                Err(e) => {
                    error!("Failed to get account for {}: {}", stake_pool_id, e);
                    continue;
                }
            }
        }
    }

    let deserialized_lst_pools_info = deserialize_pools_info(lst_pool.clone(), maybe_rpc_client);

    let stake_account_of_interest =
        get_pools_of_interest_based_on_vote_account(deserialized_lst_pools_info);

    info!(
        "spl stake pools  count {:?} account of intrest {:?}",
        lst_pool.len(),
        stake_account_of_interest.values().len()
    );
    stake_account_of_interest
}

pub fn deserialize_pools_info(
    all_lst_pools: Vec<(Pubkey, Account)>,
    maybe_rpc_client: Option<&SyncRpcClient>,
) -> Vec<(Pubkey, StakePool, ValidatorList)> {
    let mut deserialize_lst_pools: Vec<(Pubkey, StakePool, ValidatorList)> = Vec::new();
    for each in all_lst_pools.iter() {
        let stake_pool_result = try_from_slice_unchecked::<StakePool>(each.1.data.as_slice());
        match stake_pool_result {
            Ok(stake_pool) => {
                if stake_pool.account_type == spl_stake_pool::state::AccountType::StakePool {
                    let validator_list_result =
                        get_validator_list(&stake_pool.validator_list, maybe_rpc_client);
                    match validator_list_result {
                        Ok(validator_list) => {
                            deserialize_lst_pools.push((each.0, stake_pool, validator_list));
                        }
                        Err(err) => {
                            error!("Failed to get validator list for pool {}: {}", each.0, err);
                        }
                    }
                }
            }
            Err(err) => {
                error!(
                    "Failed to deserialize StakePool for pool {}: {}",
                    each.0, err
                );
            }
        }
    }

    deserialize_lst_pools
}

pub fn get_validator_list(
    validator_list_address: &Pubkey,
    maybe_rpc_client: Option<&SyncRpcClient>,
) -> Result<ValidatorList, ValidatorListError> {
    let rpc_client = maybe_rpc_client.unwrap();

    let account_data = rpc_client
        .get_account_data(validator_list_address)
        .map_err(|err| {
            error!(
                "Error fetching validator list data for {}: {}",
                validator_list_address, err
            );
            err
        })?;

    let validator_list = try_from_slice_unchecked::<ValidatorList>(account_data.as_slice())
        .map_err(|err| format!("Invalid validator list {}: {}", validator_list_address, err))?;

    Ok(validator_list)
}

pub fn get_pools_of_interest_based_on_vote_account(
    deserialized_lst_pools_info: Vec<(Pubkey, StakePool, ValidatorList)>,
) -> HashMap<Pubkey, Pubkey> {
    let mut pool_stake_account_info: HashMap<Pubkey, Pubkey> = HashMap::new();

    for (pool_pubkey, pool_info, validators_list) in deserialized_lst_pools_info.iter() {
        for each_validator in validators_list.validators.iter() {
            let (stake_account_address, _) = find_stake_program_address(
                &spl_stake_pool::id(),
                &each_validator.vote_account_address,
                pool_pubkey,
                NonZeroU32::new(each_validator.validator_seed_suffix.into()),
            );
            pool_stake_account_info.insert(stake_account_address.clone(), pool_info.reserve_stake);
        }
    }

    pool_stake_account_info
}

pub fn get_proof(merkle_tree: &MerkleTree, i: usize) -> Vec<[u8; 32]> {
    let mut proof = Vec::new();
    let path = merkle_tree.find_path(i).expect("path to index");
    for branch in path.get_proof_entries() {
        if let Some(hash) = branch.get_left_sibling() {
            proof.push(hash.to_bytes());
        } else if let Some(hash) = branch.get_right_sibling() {
            proof.push(hash.to_bytes());
        } else {
            panic!("expected some hash at each level of the tree");
        }
    }
    proof
}

#[derive(Clone, Eq, Debug, Hash, PartialEq, Deserialize, Serialize)]
pub struct TreeNode {
    #[serde(with = "pubkey_string_conversion")]
    pub stake_account_pubkey: Pubkey,

    /// The stake account entitled to redeem.
    #[serde(with = "pubkey_string_conversion")]
    pub claimant: Pubkey,

    /// Pubkey of the ClaimStatus PDA account, this account should be closed to reclaim rent.
    #[serde(with = "pubkey_string_conversion")]
    pub claim_status_pubkey: Pubkey,

    /// Bump of the ClaimStatus PDA account
    pub claim_status_bump: u8,

    #[serde(with = "pubkey_string_conversion")]
    pub staker_pubkey: Pubkey,

    #[serde(with = "pubkey_string_conversion")]
    pub withdrawer_pubkey: Pubkey,

    pub active_stake: u64,

    /// Is the staker eligible to receive block reward.
    pub is_excluded: bool,

    /// The amount this account is entitled to.
    pub amount: u64,

    ///Field for the pool reserve account to set its priority high for sending txns.
    pub priority: u8,

    /// The proof associated with this TreeNode
    pub proof: Option<Vec<[u8; 32]>>,
}

impl TreeNode {
    pub fn vec_from_stake_meta(
        stake_meta: &StakeMeta,
        stake_account_of_interest: HashMap<Pubkey, Pubkey>,
        excluded_stakers: HashSet<String>,
        reward_distribution_program_id: Pubkey,
    ) -> Result<Option<Vec<TreeNode>>, MerkleRootGeneratorError> {
        if let Some(reward_distribution_meta) = stake_meta.maybe_reward_distribution_meta.as_ref() {
            let mut tree_nodes = vec![];

            // Calculate total transactions required for claiming expenses.
            let claim_transactions = stake_meta
                .delegations
                .iter()
                .filter(|d| !excluded_stakers.contains(&d.staker_pubkey.to_string()))
                .count()
                + 1; // Includes claims for Stakers and Expenses.
            let close_claim_status_and_rda_transactions =
                (claim_transactions as f64 / CLOSE_INSTR_PER_TXN as f64).ceil() as usize + 1; // Close RDA and Claim Status Account.(One closing transaction per 4 delegations).
            let total_transactions = 1 // Upload Merkle root
                + claim_transactions
                + close_claim_status_and_rda_transactions;
            let expense_amount = (total_transactions * 5000) as u64;
            info!(
                "vote={},delegation={},excluded={},expense={}",
                stake_meta.validator_vote_account,
                stake_meta.delegations.len(),
                stake_meta.delegations.len() - claim_transactions + 1,
                expense_amount
            );
            let (expense_claim_status_pubkey, expense_claim_status_bump) =
                Pubkey::find_program_address(
                    &[
                        ClaimStatus::SEED,
                        &reward_distribution_meta
                            .merkle_root_upload_authority
                            .to_bytes(),
                        &reward_distribution_meta
                            .reward_distribution_pubkey
                            .to_bytes(),
                    ],
                    &reward_distribution_program_id,
                );
            tree_nodes.push(TreeNode {
                stake_account_pubkey: reward_distribution_meta.merkle_root_upload_authority,
                claimant: reward_distribution_meta.merkle_root_upload_authority,
                claim_status_pubkey: expense_claim_status_pubkey,
                claim_status_bump: expense_claim_status_bump,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 0,
                amount: expense_amount,
                is_excluded: false,
                priority: 0,
                proof: None,
            });

            let remaining_rewards = reward_distribution_meta
                .total_rewards
                .checked_sub(expense_amount)
                .ok_or(MerkleRootGeneratorError::MathOverflow)?
                as u128;

            let included_delegated_total: u128 = stake_meta
                .delegations
                .iter()
                .filter(|d| !excluded_stakers.contains(&d.staker_pubkey.to_string()))
                .map(|d| d.lamports_delegated as u128)
                .sum();
            tree_nodes.extend(
                stake_meta
                    .delegations
                    .iter()
                    .map(|delegation| {
                        let is_excluded =
                            excluded_stakers.contains(&delegation.staker_pubkey.to_string());
                        let amount_delegated = delegation.lamports_delegated as u128;

                        let reward_amount = if is_excluded || included_delegated_total == 0 {
                            0
                        } else {
                            amount_delegated
                                .checked_mul(remaining_rewards)
                                .ok_or(MerkleRootGeneratorError::MathOverflow)?
                                .checked_div(included_delegated_total)
                                .ok_or(MerkleRootGeneratorError::MathOverflow)?
                        };

                        let claimant_pubkey = stake_account_of_interest
                            .get(&delegation.stake_account_pubkey)
                            .cloned()
                            .unwrap_or(delegation.stake_account_pubkey);

                        let priority = if stake_account_of_interest
                            .contains_key(&delegation.stake_account_pubkey)
                        {
                            1
                        } else {
                            0
                        };

                        let (claim_status_pubkey, claim_status_bump) = Pubkey::find_program_address(
                            &[
                                ClaimStatus::SEED,
                                &claimant_pubkey.to_bytes(),
                                &reward_distribution_meta
                                    .reward_distribution_pubkey
                                    .to_bytes(),
                            ],
                            &reward_distribution_program_id,
                        );

                        Ok(TreeNode {
                            stake_account_pubkey: delegation.stake_account_pubkey,
                            claimant: claimant_pubkey,
                            claim_status_pubkey,
                            claim_status_bump,
                            staker_pubkey: delegation.staker_pubkey,
                            withdrawer_pubkey: delegation.withdrawer_pubkey,
                            active_stake: delegation.lamports_delegated,
                            amount: reward_amount as u64,
                            is_excluded,
                            priority,
                            proof: None,
                        })
                    })
                    .collect::<Result<Vec<TreeNode>, MerkleRootGeneratorError>>()?,
            );

            Ok(Some(tree_nodes))
        } else {
            Ok(None)
        }
    }

    fn hash(&self) -> Hash {
        let mut hasher = Hasher::default();
        hasher.hash(self.claimant.as_ref());
        hasher.hash(self.amount.to_le_bytes().as_ref());
        hasher.result()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StakeMetaCollection {
    /// List of [StakeMeta].
    pub stake_metas: Vec<StakeMeta>,

    /// base58 encoded reward-distribution program id.
    #[serde(with = "pubkey_string_conversion")]
    pub reward_distribution_program_id: Pubkey,

    /// Base58 encoded bank hash this object was generated at.
    pub bank_hash: String,

    /// Epoch for which this object was generated for.
    pub epoch: Epoch,

    /// Slot at which this object was generated.
    pub slot: Slot,
}

#[derive(Clone, Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct StakeMeta {
    #[serde(with = "pubkey_string_conversion")]
    pub validator_vote_account: Pubkey,

    #[serde(with = "pubkey_string_conversion")]
    pub validator_node_pubkey: Pubkey,

    /// The validator's reward-distribution meta if it exists.
    pub maybe_reward_distribution_meta: Option<RewardDistributionMeta>,

    /// Delegations to this validator.
    pub delegations: Vec<Delegation>,

    /// The total amount of delegations to the validator.
    pub total_delegated: u64,

    /// The validator's delegation commission rate as a percentage between 0-100.
    pub commission: u8,
}

impl Ord for StakeMeta {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.validator_vote_account
            .cmp(&other.validator_vote_account)
    }
}

impl PartialOrd<Self> for StakeMeta {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct RewardDistributionMeta {
    #[serde(with = "pubkey_string_conversion")]
    pub merkle_root_upload_authority: Pubkey,

    #[serde(with = "pubkey_string_conversion")]
    pub reward_distribution_pubkey: Pubkey,

    #[serde(with = "pubkey_string_conversion")]
    pub rakurai_commission_pubkey: Pubkey,

    pub rakurai_commission_bps: u16,

    /// The validator's total rewards in the [RewardCollectionAccount].
    pub total_rewards: u64,

    /// The validator's cut of rewards from [RewardCollectionAccount], calculated from the on-chain
    /// commission fee bps.
    pub validator_fee_bps: u16,
}

impl RewardDistributionMeta {
    fn from_rda_wrapper(
        rda_wrapper: RewardCollectionAccountWrapper,
        // The amount that will be left remaining in the rda to maintain rent exemption status.
        rent_exempt_amount: u64,
    ) -> Result<Self, stake_meta_generator_workflow::StakeMetaGeneratorError> {
        Ok(RewardDistributionMeta {
            reward_distribution_pubkey: rda_wrapper.reward_distribution_pubkey,
            total_rewards: rda_wrapper
                .account_data
                .lamports()
                .checked_sub(rent_exempt_amount)
                .ok_or(CheckedMathError)?,
            rakurai_commission_pubkey: rda_wrapper
                .reward_distribution_account
                .rakurai_commission_account,
            rakurai_commission_bps: rda_wrapper
                .reward_distribution_account
                .rakurai_commission_bps,
            validator_fee_bps: rda_wrapper
                .reward_distribution_account
                .validator_commission_bps,
            merkle_root_upload_authority: rda_wrapper
                .reward_distribution_account
                .merkle_root_upload_authority,
        })
    }
}

#[derive(Clone, Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct Delegation {
    #[serde(with = "pubkey_string_conversion")]
    pub stake_account_pubkey: Pubkey,

    #[serde(with = "pubkey_string_conversion")]
    pub staker_pubkey: Pubkey,

    #[serde(with = "pubkey_string_conversion")]
    pub withdrawer_pubkey: Pubkey,

    /// Lamports delegated by the stake account
    pub lamports_delegated: u64,
}

impl Ord for Delegation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (
            self.stake_account_pubkey,
            self.withdrawer_pubkey,
            self.staker_pubkey,
            self.lamports_delegated,
        )
            .cmp(&(
                other.stake_account_pubkey,
                other.withdrawer_pubkey,
                other.staker_pubkey,
                other.lamports_delegated,
            ))
    }
}

impl PartialOrd<Self> for Delegation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Convenience wrapper around [RewardCollectionAccount]
pub struct RewardCollectionAccountWrapper {
    pub reward_distribution_account: RewardCollectionAccount,
    pub account_data: AccountSharedData,
    pub reward_distribution_pubkey: Pubkey,
}

// TODO: move to program's sdk
pub fn derive_reward_distribution_account_address(
    reward_distribution_program_id: &Pubkey,
    vote_pubkey: &Pubkey,
    epoch: Epoch,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            RewardCollectionAccount::SEED,
            vote_pubkey.to_bytes().as_ref(),
            epoch.to_le_bytes().as_ref(),
        ],
        reward_distribution_program_id,
    )
}

pub const MAX_RETRIES: usize = 5;
pub const FAIL_DELAY: Duration = Duration::from_millis(100);

pub async fn sign_and_send_transactions_with_retries(
    signer: &Keypair,
    rpc_client: &RpcClient,
    max_concurrent_rpc_get_reqs: usize,
    transactions: Vec<Transaction>,
    txn_send_batch_size: usize,
    max_loop_duration: Duration,
) -> (Vec<Transaction>, HashMap<Signature, Error>) {
    let semaphore = Arc::new(Semaphore::new(max_concurrent_rpc_get_reqs));
    let mut errors = HashMap::default();
    let mut blockhash = rpc_client
        .get_latest_blockhash()
        .await
        .expect("fetch latest blockhash");
    // track unsigned txns
    let mut transactions_to_process = transactions
        .into_iter()
        .map(|txn| (txn.message_data(), txn))
        .collect::<HashMap<Vec<u8>, Transaction>>();

    let start = Instant::now();
    while start.elapsed() < max_loop_duration && !transactions_to_process.is_empty() {
        // ensure we always have a recent blockhash
        // blockhashes last max 150 blocks
        // finalized commitment is ~32 slots behind rewards
        // assuming 0% skip rate (every slot has a block), we’d have roughly 120 slots
        // or (120*0.4s) = 48s to land a tx before it expires
        // if we’re refreshing every 30s, then any txs sent immediately before the refresh would likely expire
        if start.elapsed() > Duration::from_secs(1) {
            blockhash = rpc_client
                .get_latest_blockhash()
                .await
                .expect("fetch latest blockhash");
        }
        info!(
            "Sending {txn_send_batch_size} of {} transactions to claim rewards",
            transactions_to_process.len()
        );
        let send_futs = transactions_to_process
            .iter()
            .take(txn_send_batch_size)
            .map(|(hash, txn)| {
                let semaphore = semaphore.clone();
                async move {
                    let _permit = semaphore.acquire_owned().await.unwrap(); // wait until our turn
                    let (txn, res) = signed_send(signer, rpc_client, blockhash, txn.clone()).await;
                    (hash.clone(), txn, res)
                }
            });

        let send_res = futures::future::join_all(send_futs).await;
        let new_errors = send_res
            .into_iter()
            .filter_map(|(hash, txn, result)| match result {
                Err(e) => Some((txn.signatures[0], e)),
                Ok(..) => {
                    let _ = transactions_to_process.remove(&hash);
                    None
                }
            })
            .collect::<HashMap<_, _>>();

        errors.extend(new_errors);
    }

    (transactions_to_process.values().cloned().collect(), errors)
}

pub async fn send_until_blockhash_expires(
    rpc_client: &RpcClient,
    transactions: Vec<(Signature, Transaction)>,
    blockhash: Hash,
) -> solana_rpc_client_api::client_error::Result<((), Vec<(Signature, u64)>)> {
    let mut claim_transactions = transactions;
    let txs_requesting_send = claim_transactions.len();
    let mut processed_signatures = Vec::new();

    while rpc_client
        .is_blockhash_valid(&blockhash, CommitmentConfig::processed())
        .await?
    {
        let mut check_signatures = HashSet::with_capacity(claim_transactions.len());
        let mut already_processed = HashSet::with_capacity(claim_transactions.len());
        let mut is_blockhash_not_found = false;

        for (signature, tx) in &claim_transactions {
            match rpc_client
                .send_transaction_with_config(
                    tx,
                    RpcSendTransactionConfig {
                        skip_preflight: false,
                        preflight_commitment: Some(CommitmentLevel::Confirmed),
                        max_retries: Some(2),
                        ..RpcSendTransactionConfig::default()
                    },
                )
                .await
            {
                Ok(_) => {
                    check_signatures.insert(*signature);
                }
                Err(e) => match e.get_transaction_error() {
                    Some(TransactionError::BlockhashNotFound) => {
                        is_blockhash_not_found = true;
                        break;
                    }
                    Some(TransactionError::AlreadyProcessed) => {
                        already_processed.insert(*tx.get_signature());
                    }
                    Some(e) => {
                        warn!(
                            "TransactionError sending signature: {} error: {:?} tx: {:?}",
                            tx.get_signature(),
                            e,
                            tx
                        );
                    }
                    None => {
                        warn!(
                            "Unknown error sending transaction signature: {} error: {:?}",
                            tx.get_signature(),
                            e,
                        );
                    }
                },
            }
        }

        sleep(Duration::from_secs(10)).await;

        let signatures: Vec<Signature> = check_signatures.iter().cloned().collect();
        let statuses = get_batched_signatures_statuses(rpc_client, &signatures).await?;

        for (signature, maybe_status) in &statuses {
            if let Some(_status) = maybe_status {
                claim_transactions.retain(|(sig, _)| sig != signature);
                check_signatures.retain(|sig| sig != signature);
                processed_signatures.push((*signature, _status.slot));
            }
        }

        info!("Remaining txns{:?}", claim_transactions);

        for signature in already_processed {
            claim_transactions.retain(|(sig, _)| sig != &signature);
        }

        if claim_transactions.is_empty() || is_blockhash_not_found {
            break;
        }
    }

    let num_landed = txs_requesting_send
        .checked_sub(claim_transactions.len())
        .unwrap();
    info!("num_landed: {:?}", num_landed);

    Ok(((), processed_signatures))
}

pub async fn get_batched_signatures_statuses(
    rpc_client: &RpcClient,
    signatures: &[Signature],
) -> solana_rpc_client_api::client_error::Result<Vec<(Signature, Option<TransactionStatus>)>> {
    let mut signature_statuses = Vec::new();

    for signatures_batch in signatures.chunks(100) {
        // was using get_signature_statuses_with_history, but it blocks if the signatures don't exist
        // bigtable calls to read signatures that don't exist block forever w/o --rpc-bigtable-timeout argument set
        // get_signature_statuses looks in status_cache, which only has a 150 block history
        // may have false negative, but for this workflow it doesn't matter
        let statuses = rpc_client.get_signature_statuses(signatures_batch).await?;
        signature_statuses.extend(signatures_batch.iter().cloned().zip(statuses.value));
    }
    Ok(signature_statuses)
}

/// Just in time sign and send transaction to RPC
async fn signed_send(
    signer: &Keypair,
    rpc_client: &RpcClient,
    blockhash: Hash,
    mut txn: Transaction,
) -> (Transaction, solana_rpc_client_api::client_error::Result<()>) {
    txn.sign(&[signer], blockhash); // just in time signing
    let res = match rpc_client.send_and_confirm_transaction(&txn).await {
        Ok(_) => Ok(()),
        Err(e) => {
            match e.kind {
                // Already claimed, skip.
                ErrorKind::TransactionError(TransactionError::AlreadyProcessed)
                | ErrorKind::TransactionError(TransactionError::InstructionError(
                    0,
                    InstructionError::Custom(0),
                ))
                | ErrorKind::RpcError(RpcError::RpcResponseError {
                    data:
                        RpcResponseErrorData::SendTransactionPreflightFailure(
                            RpcSimulateTransactionResult {
                                err:
                                    Some(TransactionError::InstructionError(
                                        0,
                                        InstructionError::Custom(0),
                                    )),
                                ..
                            },
                        ),
                    ..
                }) => Ok(()),

                // transaction got held up too long and blockhash expired. retry txn
                ErrorKind::TransactionError(TransactionError::BlockhashNotFound) => Err(e),

                // unexpected error, warn and retry
                _ => {
                    error!(
                        "Error sending transaction. Signature: {}, Error: {e:?}",
                        txn.signatures[0]
                    );
                    Err(e)
                }
            }
        }
    };

    (txn, res)
}

async fn get_batched_accounts(
    rpc_client: &RpcClient,
    pubkeys: &[Pubkey],
) -> solana_rpc_client_api::client_error::Result<HashMap<Pubkey, Option<Account>>> {
    let mut batched_accounts = HashMap::new();

    for pubkeys_chunk in pubkeys.chunks(MAX_MULTIPLE_ACCOUNTS) {
        let accounts = rpc_client.get_multiple_accounts(pubkeys_chunk).await?;
        batched_accounts.extend(pubkeys_chunk.iter().cloned().zip(accounts));
    }
    Ok(batched_accounts)
}

mod pubkey_string_conversion {
    use {
        serde::{self, Deserialize, Deserializer, Serializer},
        solana_sdk::pubkey::Pubkey,
        std::str::FromStr,
    };

    pub(crate) fn serialize<S>(pubkey: &Pubkey, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&pubkey.to_string())
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Pubkey, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Pubkey::from_str(&s).map_err(serde::de::Error::custom)
    }
}

pub fn read_json_from_file<T>(path: &PathBuf) -> serde_json::Result<T>
where
    T: DeserializeOwned,
{
    let file = File::open(path).unwrap();
    let reader = BufReader::new(file);
    serde_json::from_reader(reader)
}

#[cfg(test)]
mod tests {
    use {super::*, block_reward_distribution::merkle_proof};

    #[test]
    fn test_merkle_tree_verify() {
        // Create the merkle tree and proofs
        let rda = Pubkey::new_unique();
        let (acct_0, acct_1) = (Pubkey::new_unique(), Pubkey::new_unique());
        let claim_statuses = &[(acct_0, rda), (acct_1, rda)]
            .iter()
            .map(|(claimant, rda)| {
                Pubkey::find_program_address(
                    &[ClaimStatus::SEED, &claimant.to_bytes(), &rda.to_bytes()],
                    &RewardDistribution::id(),
                )
            })
            .collect::<Vec<(Pubkey, u8)>>();
        let tree_nodes = vec![
            TreeNode {
                stake_account_pubkey: acct_0,
                claimant: acct_0,
                claim_status_pubkey: claim_statuses[0].0,
                claim_status_bump: claim_statuses[0].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 207_108,
                amount: 151_507,
                is_excluded: true,
                priority: 1,
                proof: None,
            },
            TreeNode {
                stake_account_pubkey: acct_1,
                claimant: acct_1,
                claim_status_pubkey: claim_statuses[1].0,
                claim_status_bump: claim_statuses[1].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 331_133,
                amount: 176_624,
                is_excluded: true,
                priority: 1,
                proof: None,
            },
        ];

        // First the nodes are hashed and merkle tree constructed
        let hashed_nodes: Vec<[u8; 32]> = tree_nodes.iter().map(|n| n.hash().to_bytes()).collect();
        let mk = MerkleTree::new(&hashed_nodes[..], true);
        let root = mk.get_root().expect("to have valid root").to_bytes();

        // verify first node
        let node = solana_program::hash::hashv(&[&[0u8], &hashed_nodes[0]]);
        let proof = get_proof(&mk, 0);
        assert!(merkle_proof::verify(proof, root, node.to_bytes()));

        // verify second node
        let node = solana_program::hash::hashv(&[&[0u8], &hashed_nodes[1]]);
        let proof = get_proof(&mk, 1);
        assert!(merkle_proof::verify(proof, root, node.to_bytes()));
    }

    #[test]
    fn test_new_from_stake_meta_collection_happy_path() {
        let merkle_root_upload_authority = Pubkey::new_unique();

        let (rda_0, rda_1) = (Pubkey::new_unique(), Pubkey::new_unique());

        let stake_account_0 = Pubkey::new_unique();
        let stake_account_1 = Pubkey::new_unique();
        let stake_account_2 = Pubkey::new_unique();
        let stake_account_3 = Pubkey::new_unique();

        let staker_account_0 = Pubkey::new_unique();
        let staker_account_1 = Pubkey::new_unique();
        let staker_account_2 = Pubkey::new_unique();
        let staker_account_3 = Pubkey::new_unique();

        let validator_vote_account_0 = Pubkey::new_unique();
        let validator_vote_account_1 = Pubkey::new_unique();

        let validator_id_0 = Pubkey::new_unique();
        let validator_id_1 = Pubkey::new_unique();

        let rakurai_commission_pubkey = Pubkey::new_unique();
        let rakurai_commission_bps = 1_000;

        let stake_meta_collection = StakeMetaCollection {
            stake_metas: vec![
                StakeMeta {
                    validator_vote_account: validator_vote_account_0,
                    validator_node_pubkey: validator_id_0,
                    maybe_reward_distribution_meta: Some(RewardDistributionMeta {
                        merkle_root_upload_authority,
                        reward_distribution_pubkey: rda_0,
                        rakurai_commission_pubkey,
                        rakurai_commission_bps,

                        total_rewards: 1_900_122_111_000,
                        validator_fee_bps: 100,
                    }),
                    delegations: vec![
                        Delegation {
                            stake_account_pubkey: stake_account_0,
                            staker_pubkey: staker_account_0,
                            withdrawer_pubkey: staker_account_0,
                            lamports_delegated: 123_999_123_555,
                        },
                        Delegation {
                            stake_account_pubkey: stake_account_1,
                            staker_pubkey: staker_account_1,
                            withdrawer_pubkey: staker_account_1,
                            lamports_delegated: 144_555_444_556,
                        },
                    ],
                    total_delegated: 1_555_123_000_333_454_000,
                    commission: 100,
                },
                StakeMeta {
                    validator_vote_account: validator_vote_account_1,
                    validator_node_pubkey: validator_id_1,
                    maybe_reward_distribution_meta: Some(RewardDistributionMeta {
                        merkle_root_upload_authority,
                        reward_distribution_pubkey: rda_1,
                        rakurai_commission_pubkey,
                        rakurai_commission_bps,
                        total_rewards: 1_900_122_111_333,
                        validator_fee_bps: 200,
                    }),
                    delegations: vec![
                        Delegation {
                            stake_account_pubkey: stake_account_2,
                            staker_pubkey: staker_account_2,
                            withdrawer_pubkey: staker_account_2,
                            lamports_delegated: 224_555_444,
                        },
                        Delegation {
                            stake_account_pubkey: stake_account_3,
                            staker_pubkey: staker_account_3,
                            withdrawer_pubkey: staker_account_3,
                            lamports_delegated: 700_888_944_555,
                        },
                    ],
                    total_delegated: 2_565_318_909_444_123,
                    commission: 10,
                },
            ],
            reward_distribution_program_id: Pubkey::new_unique(),
            bank_hash: Hash::new_unique().to_string(),
            epoch: 100,
            slot: 2_000_000,
        };

        let merkle_tree_collection = GeneratedMerkleTreeCollection::new_from_stake_meta_collection(
            stake_meta_collection.clone(),
            None,
        )
        .unwrap();

        assert_eq!(stake_meta_collection.epoch, merkle_tree_collection.epoch);
        assert_eq!(
            stake_meta_collection.bank_hash,
            merkle_tree_collection.bank_hash
        );
        assert_eq!(stake_meta_collection.slot, merkle_tree_collection.slot);
        assert_eq!(
            stake_meta_collection.stake_metas.len(),
            merkle_tree_collection.generated_merkle_trees.len()
        );
        let claim_statuses = &[
            (merkle_root_upload_authority, rda_0),
            (rakurai_commission_wallet, rda_0),
            (validator_vote_account_0, rda_0),
            (stake_account_0, rda_0),
            (stake_account_1, rda_0),
            (merkle_root_upload_authority, rda_1),
            (rakurai_commission_wallet, rda_1),
            (validator_vote_account_1, rda_1),
            (stake_account_2, rda_1),
            (stake_account_3, rda_1),
        ]
        .iter()
        .map(|(claimant, rda)| {
            Pubkey::find_program_address(
                &[ClaimStatus::SEED, &claimant.to_bytes(), &rda.to_bytes()],
                &RewardDistribution::id(),
            )
        })
        .collect::<Vec<(Pubkey, u8)>>();
        let tree_nodes = vec![
            TreeNode {
                stake_account_pubkey: merkle_root_upload_authority,
                claimant: merkle_root_upload_authority,
                claim_status_pubkey: claim_statuses[2].0,
                claim_status_bump: claim_statuses[2].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 217_118,
                amount: 45_000,
                is_excluded: true,
                priority: 0,
                proof: None,
            },
            TreeNode {
                stake_account_pubkey: rakurai_commission_wallet,
                claimant: rakurai_commission_wallet,
                claim_status_pubkey: claim_statuses[2].0,
                claim_status_bump: claim_statuses[2].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 217_118,
                amount: 190_012_206_600,
                is_excluded: true,
                priority: 0,
                proof: None,
            },
            TreeNode {
                stake_account_pubkey: validator_vote_account_0,
                claimant: validator_vote_account_0,
                claim_status_pubkey: claim_statuses[0].0,
                claim_status_bump: claim_statuses[0].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 120_199_133,
                amount: 17_101_098_594,
                is_excluded: true,
                priority: 0,
                proof: None,
            },
            TreeNode {
                stake_account_pubkey: stake_account_0,
                claimant: stake_account_0,
                claim_status_pubkey: claim_statuses[1].0,
                claim_status_bump: claim_statuses[1].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 297_309_110,
                amount: 134_993,
                is_excluded: true,
                priority: 0,
                proof: None,
            },
            TreeNode {
                stake_account_pubkey: stake_account_1,
                claimant: stake_account_1,
                claim_status_pubkey: claim_statuses[2].0,
                claim_status_bump: claim_statuses[2].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 217_118,
                amount: 157_372,
                is_excluded: true,
                priority: 0,
                proof: None,
            },
        ];
        let hashed_nodes: Vec<[u8; 32]> = tree_nodes.iter().map(|n| n.hash().to_bytes()).collect();
        let merkle_tree = MerkleTree::new(&hashed_nodes[..], true);
        let gmt_0 = GeneratedMerkleTree {
            validator_vote_account: validator_vote_account_0,
            validator_node_pubkey: validator_id_0,
            reward_distribution_account: rda_0,
            merkle_root_upload_authority,
            merkle_root: *merkle_tree.get_root().unwrap(),
            tree_nodes,
            total_delegated: 123_676,
            validator_commission_bps: 1000,
            voting_commission: 100,
            max_total_claim: stake_meta_collection.stake_metas[0]
                .clone()
                .maybe_reward_distribution_meta
                .unwrap()
                .total_rewards,
            max_num_nodes: 5,
            rakurai_commission_bps,
        };

        let tree_nodes = vec![
            TreeNode {
                stake_account_pubkey: merkle_root_upload_authority,
                claimant: merkle_root_upload_authority,
                claim_status_pubkey: claim_statuses[2].0,
                claim_status_bump: claim_statuses[2].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 217_118,
                amount: 45_000,
                is_excluded: true,
                priority: 0,
                proof: None,
            },
            TreeNode {
                stake_account_pubkey: rakurai_commission_wallet,
                claimant: rakurai_commission_wallet,
                claim_status_pubkey: claim_statuses[2].0,
                claim_status_bump: claim_statuses[2].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 217_118,
                amount: 190_012_206_633,
                is_excluded: true,
                priority: 0,
                proof: None,
            },
            TreeNode {
                stake_account_pubkey: validator_vote_account_1,
                claimant: validator_vote_account_1,
                claim_status_pubkey: claim_statuses[3].0,
                claim_status_bump: claim_statuses[3].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 297_309_110,
                amount: 34_202_197_194,
                is_excluded: true,
                priority: 0,
                proof: None,
            },
            TreeNode {
                stake_account_pubkey: stake_account_2,
                claimant: stake_account_2,
                claim_status_pubkey: claim_statuses[4].0,
                claim_status_bump: claim_statuses[4].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 297_309_110,
                amount: 146_700,
                is_excluded: true,
                priority: 0,
                proof: None,
            },
            TreeNode {
                stake_account_pubkey: stake_account_3,
                claimant: stake_account_3,
                claim_status_pubkey: claim_statuses[5].0,
                claim_status_bump: claim_statuses[5].1,
                staker_pubkey: Pubkey::default(),
                withdrawer_pubkey: Pubkey::default(),
                active_stake: 297_309_110,
                amount: 457_886_599,
                is_excluded: true,
                priority: 0,
                proof: None,
            },
        ];
        let hashed_nodes: Vec<[u8; 32]> = tree_nodes.iter().map(|n| n.hash().to_bytes()).collect();
        let merkle_tree = MerkleTree::new(&hashed_nodes[..], true);
        let gmt_1 = GeneratedMerkleTree {
            validator_vote_account: validator_vote_account_1,
            validator_node_pubkey: validator_id_1,
            reward_distribution_account: rda_1,
            merkle_root_upload_authority,
            merkle_root: *merkle_tree.get_root().unwrap(),
            tree_nodes,
            total_delegated: 123_676,
            validator_commission_bps: 1000,
            voting_commission: 100,
            max_total_claim: stake_meta_collection.stake_metas[1]
                .clone()
                .maybe_reward_distribution_meta
                .unwrap()
                .total_rewards,
            max_num_nodes: 5,
            rakurai_commission_bps,
        };

        let expected_generated_merkle_trees = vec![gmt_0, gmt_1];
        let actual_generated_merkle_trees = merkle_tree_collection.generated_merkle_trees;

        expected_generated_merkle_trees
            .iter()
            .for_each(|expected_gmt| {
                let actual_gmt = actual_generated_merkle_trees
                    .iter()
                    .find(|gmt| {
                        gmt.reward_distribution_account == expected_gmt.reward_distribution_account
                    })
                    .unwrap();

                assert_eq!(expected_gmt.max_num_nodes, actual_gmt.max_num_nodes);
                assert_eq!(expected_gmt.max_total_claim, actual_gmt.max_total_claim);
                assert_eq!(
                    expected_gmt.reward_distribution_account,
                    actual_gmt.reward_distribution_account
                );
                assert_eq!(expected_gmt.tree_nodes.len(), actual_gmt.tree_nodes.len());
                expected_gmt
                    .tree_nodes
                    .iter()
                    .for_each(|expected_tree_node| {
                        let actual_tree_node = actual_gmt
                            .tree_nodes
                            .iter()
                            .find(|tree_node| tree_node.claimant == expected_tree_node.claimant)
                            .unwrap();
                        assert_eq!(expected_tree_node.amount, actual_tree_node.amount);
                    });
                assert_eq!(expected_gmt.merkle_root, actual_gmt.merkle_root);
            });
    }
}
