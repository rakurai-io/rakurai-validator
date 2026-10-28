use {
    crate::{
        RewardCollectionAccount, RewardCollectionAccountWrapper, RewardDistributionMeta, StakeMeta,
        StakeMetaCollection, derive_reward_distribution_account_address,
    },
    agave_snapshots::{SnapshotInterval, snapshot_config::SnapshotConfig},
    anchor_lang::AccountDeserialize,
    itertools::Itertools,
    log::*,
    solana_client::client_error::ClientError,
    solana_genesis_utils::{
        MAX_GENESIS_ARCHIVE_UNPACKED_SIZE, OpenGenesisConfigError, open_genesis_config,
    },
    solana_ledger::{
        bank_forks_utils::{self, BankForksUtilsError},
        blockstore::BlockstoreError,
        blockstore_processor::{BlockstoreProcessorError, ProcessOptions},
    },
    solana_runtime::{bank::Bank, stakes::StakeAccount},
    solana_account::ReadableAccount,
    solana_pubkey::Pubkey,
    solana_sdk::clock::Slot,
    solana_stake_interface::stake_history::StakeHistory,
    solana_vote::vote_account::VoteAccount,
    std::{
        collections::HashMap,
        fmt::{Debug, Display, Formatter},
        fs::File,
        io::{BufWriter, Write},
        mem::size_of,
        num::NonZeroU64,
        path::{Path, PathBuf},
        sync::{Arc, atomic::AtomicBool},
    },
    thiserror::Error,
};

#[derive(Error, Debug)]
pub enum StakeMetaGeneratorError {
    #[error(transparent)]
    AnchorError(#[from] Box<anchor_lang::error::Error>),

    #[error(transparent)]
    BlockstoreError(#[from] BlockstoreError),

    #[error(transparent)]
    BlockstoreProcessorError(#[from] BlockstoreProcessorError),

    #[error(transparent)]
    IoError(#[from] std::io::Error),

    CheckedMathError,

    #[error(transparent)]
    RpcError(#[from] ClientError),

    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),

    SnapshotSlotNotFound,

    BankForksUtilsError(#[from] BankForksUtilsError),
    GenesisConfigError(#[from] OpenGenesisConfigError),
}

impl Display for StakeMetaGeneratorError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self, f)
    }
}

/// Runs the entire workflow of creating a bank from a snapshot to writing stake meta-data
/// to a JSON file.
pub fn generate_stake_meta(
    ledger_path: &Path,
    snapshot_slot: &Slot,
    reward_distribution_program_id: &Pubkey,
    out_path: &str,
) -> Result<(), StakeMetaGeneratorError> {
    info!("Creating bank from ledger path...");
    let bank = create_bank_from_snapshot(ledger_path, snapshot_slot)?;

    info!("Generating stake_meta_collection object...");
    let stake_meta_coll = generate_stake_meta_collection(&bank, reward_distribution_program_id)?;

    info!("Writing stake_meta_collection to JSON {}...", out_path);
    write_to_json_file(&stake_meta_coll, out_path)?;

    Ok(())
}

fn create_bank_from_snapshot(
    ledger_path: &Path,
    snapshot_slot: &Slot,
) -> Result<Arc<Bank>, StakeMetaGeneratorError> {
    let genesis_config = open_genesis_config(ledger_path, MAX_GENESIS_ARCHIVE_UNPACKED_SIZE)?;
    let interval = SnapshotInterval::Slots(NonZeroU64::new(Slot::MAX).unwrap());
    let snapshot_config = SnapshotConfig {
        full_snapshot_archive_interval: interval,
        incremental_snapshot_archive_interval: interval,
        full_snapshot_archives_dir: PathBuf::from(ledger_path),
        incremental_snapshot_archives_dir: PathBuf::from(ledger_path),
        bank_snapshots_dir: PathBuf::from(ledger_path),
        ..SnapshotConfig::default()
    };

    let bank_hashes = bank_forks_utils::try_load_bank_forks_from_snapshot(
        &genesis_config,
        &[PathBuf::from(ledger_path).join(Path::new("stake-meta.accounts"))],
        &snapshot_config,
        &ProcessOptions::default(),
        None,
        Arc::new(AtomicBool::new(false)),
    )?;

    let working_bank = bank_hashes
        .as_ref()
        .unwrap()
        .0
        .read()
        .unwrap()
        .working_bank();
    assert_eq!(
        working_bank.slot(),
        *snapshot_slot,
        "expected working bank slot {}, found {}",
        snapshot_slot,
        working_bank.slot()
    );

    Ok(working_bank)
}

fn write_to_json_file(
    stake_meta_coll: &StakeMetaCollection,
    out_path: &str,
) -> Result<(), StakeMetaGeneratorError> {
    let file = File::create(out_path)?;
    let mut writer = BufWriter::new(file);
    let json = serde_json::to_string_pretty(&stake_meta_coll).unwrap();
    writer.write_all(json.as_bytes())?;
    writer.flush()?;

    Ok(())
}

/// Creates a collection of [StakeMeta]'s from the given bank.
pub fn generate_stake_meta_collection(
    bank: &Arc<Bank>,
    reward_distribution_program_id: &Pubkey,
) -> Result<StakeMetaCollection, StakeMetaGeneratorError> {
    assert!(bank.is_frozen());

    let epoch_vote_accounts = bank.epoch_vote_accounts(bank.epoch()).unwrap_or_else(|| {
        panic!(
            "No epoch_vote_accounts found for slot {} at epoch {}",
            bank.slot(),
            bank.epoch()
        )
    });

    let l_stakes = bank.stakes_cache.stakes();
    let delegations = l_stakes.stake_delegations();

    let voter_pubkey_to_delegations = group_delegations_by_voter_pubkey(delegations, bank);

    let vote_pk_and_maybe_rdas: Vec<(
        (Pubkey, &VoteAccount),
        Option<RewardCollectionAccountWrapper>,
    )> = epoch_vote_accounts
        .iter()
        .map(|(vote_pubkey, (_total_stake, vote_account))| {
            let reward_distribution_pubkey = derive_reward_distribution_account_address(
                reward_distribution_program_id,
                vote_pubkey,
                bank.epoch(),
            )
            .0;
            let rda = if let Some(account_data) = bank.get_account(&reward_distribution_pubkey) {
                if let Ok(reward_distribution_account) =
                    RewardCollectionAccount::try_deserialize(&mut account_data.data())
                {
                    Some(RewardCollectionAccountWrapper {
                        reward_distribution_account,
                        account_data,
                        reward_distribution_pubkey,
                    })
                } else {
                    None
                }
            } else {
                None
            };
            Ok(((*vote_pubkey, vote_account), rda))
        })
        .collect::<Result<_, StakeMetaGeneratorError>>()?;

    let mut stake_metas = vec![];
    for ((vote_pubkey, vote_account), maybe_rda) in vote_pk_and_maybe_rdas {
        if let Some(mut delegations) = voter_pubkey_to_delegations.get(&vote_pubkey).cloned() {
            let total_delegated = delegations.iter().fold(0u64, |sum, delegation| {
                sum.checked_add(delegation.lamports_delegated).unwrap()
            });

            let maybe_reward_distribution_meta = if let Some(rda) = maybe_rda {
                let actual_len = rda.account_data.data().len();
                let expected_len = 8_usize.saturating_add(size_of::<RewardCollectionAccount>());
                if actual_len != expected_len {
                    warn!("len mismatch actual={actual_len}, expected={expected_len}");
                }
                let rent_exempt_amount =
                    bank.get_minimum_balance_for_rent_exemption(rda.account_data.data().len());

                Some(RewardDistributionMeta::from_rda_wrapper(
                    rda,
                    rent_exempt_amount,
                )?)
            } else {
                None
            };

            let vote_state = vote_account.vote_state_view();
            delegations.sort();
            stake_metas.push(StakeMeta {
                maybe_reward_distribution_meta,
                validator_node_pubkey: *vote_state.node_pubkey(),
                validator_vote_account: vote_pubkey,
                delegations,
                total_delegated,
                commission: vote_state.commission(),
            });
        } else {
            warn!(
                "voter_pubkey not found in voter_pubkey_to_delegations map [validator_vote_pubkey={}]",
                vote_pubkey
            );
        }
    }
    stake_metas.sort();

    Ok(StakeMetaCollection {
        stake_metas,
        reward_distribution_program_id: *reward_distribution_program_id,
        bank_hash: bank.hash().to_string(),
        epoch: bank.epoch(),
        slot: bank.slot(),
    })
}

/// Given an [EpochStakes] object, return delegations grouped by voter_pubkey (validator delegated to).
fn group_delegations_by_voter_pubkey(
    delegations: &imbl::HashMap<Pubkey, StakeAccount>,
    bank: &Bank,
) -> HashMap<Pubkey, Vec<crate::Delegation>> {
    let stake_history: StakeHistory = bincode::deserialize(
        bank.get_account(&solana_sdk_ids::sysvar::stake_history::id())
            .unwrap()
            .data(),
    )
    .unwrap();

    delegations
        .iter()
        .filter(|(_stake_pubkey, stake_account)| {
            stake_account.delegation().stake_v2(
                bank.epoch(),
                &stake_history,
                bank.new_warmup_cooldown_rate_epoch(),
            ) > 0
        })
        .into_group_map_by(|(_stake_pubkey, stake_account)| stake_account.delegation().voter_pubkey)
        .into_iter()
        .map(|(voter_pubkey, group)| {
            (
                voter_pubkey,
                group
                    .into_iter()
                    .map(|(stake_pubkey, stake_account)| crate::Delegation {
                        stake_account_pubkey: *stake_pubkey,
                        staker_pubkey: stake_account
                            .stake_state()
                            .authorized()
                            .map(|a| a.staker)
                            .unwrap_or_default(),
                        withdrawer_pubkey: stake_account
                            .stake_state()
                            .authorized()
                            .map(|a| a.withdrawer)
                            .unwrap_or_default(),
                        lamports_delegated: stake_account.delegation().stake,
                    })
                    .collect::<Vec<crate::Delegation>>(),
            )
        })
        .collect()
}
