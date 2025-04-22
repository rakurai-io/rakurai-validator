use {
    crate::{
        derive_reward_distribution_account_address, RewardCollectionAccount,
        RewardCollectionAccountWrapper, RewardDistributionMeta, StakeMeta, StakeMetaCollection,
    },
    anchor_lang::AccountDeserialize,
    itertools::Itertools,
    log::*,
    solana_accounts_db::hardened_unpack::{
        open_genesis_config, OpenGenesisConfigError, MAX_GENESIS_ARCHIVE_UNPACKED_SIZE,
    },
    solana_client::client_error::ClientError,
    solana_ledger::{
        bank_forks_utils,
        bank_forks_utils::BankForksUtilsError,
        blockstore::{
            default_num_compaction_threads, default_num_flush_threads, Blockstore, BlockstoreError,
        },
        blockstore_options::{AccessType, BlockstoreOptions, LedgerColumnOptions},
        blockstore_processor::{BlockstoreProcessorError, ProcessOptions},
    },
    solana_program::{stake_history::StakeHistory, sysvar},
    solana_runtime::{bank::Bank, snapshot_config::SnapshotConfig, stakes::StakeAccount},
    solana_sdk::{
        account::{from_account, ReadableAccount},
        clock::Slot,
        pubkey::Pubkey,
    },
    solana_vote::vote_account::VoteAccount,
    std::{
        collections::HashMap,
        fmt::{Debug, Display, Formatter},
        fs::File,
        io::{BufWriter, Write},
        mem::size_of,
        path::{Path, PathBuf},
        sync::{atomic::AtomicBool, Arc},
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
    let snapshot_config = SnapshotConfig {
        full_snapshot_archive_interval_slots: Slot::MAX,
        incremental_snapshot_archive_interval_slots: Slot::MAX,
        full_snapshot_archives_dir: PathBuf::from(ledger_path),
        incremental_snapshot_archives_dir: PathBuf::from(ledger_path),
        bank_snapshots_dir: PathBuf::from(ledger_path),
        ..SnapshotConfig::default()
    };
    let blockstore = Blockstore::open_with_options(
        ledger_path,
        BlockstoreOptions {
            access_type: AccessType::Secondary,
            recovery_mode: None,
            enforce_ulimit_nofile: false,
            column_options: LedgerColumnOptions::default(),
            num_rocksdb_compaction_threads: default_num_compaction_threads(),
            num_rocksdb_flush_threads: default_num_flush_threads(),
        },
    )?;
    let (bank_forks, _, _) = bank_forks_utils::load_bank_forks(
        &genesis_config,
        &blockstore,
        vec![PathBuf::from(ledger_path).join(Path::new("stake-meta.accounts"))],
        &snapshot_config,
        &ProcessOptions::default(),
        None,
        None,
        None,
        Arc::new(AtomicBool::new(false)),
        false,
    )?;

    let working_bank = bank_forks.read().unwrap().working_bank();
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
    delegations: &im::HashMap<Pubkey, StakeAccount>,
    bank: &Bank,
) -> HashMap<Pubkey, Vec<crate::Delegation>> {
    delegations
        .into_iter()
        .filter(|(_stake_pubkey, stake_account)| {
            stake_account.delegation().stake(
                bank.epoch(),
                &from_account::<StakeHistory, _>(
                    &bank.get_account(&sysvar::stake_history::id()).unwrap(),
                )
                .unwrap(),
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

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::derive_reward_distribution_account_address,
        anchor_lang::AccountSerialize,
        reward_distribution::state::RewardCollectionAccount,
        solana_runtime::genesis_utils::{
            create_genesis_config_with_vote_accounts, GenesisConfigInfo, ValidatorVoteKeypairs,
        },
        solana_sdk::{
            self,
            account::{from_account, AccountSharedData},
            message::Message,
            signature::{Keypair, Signer},
            stake::{
                self,
                state::{Authorized, Lockup},
            },
            stake_history::StakeHistory,
            sysvar,
            transaction::Transaction,
        },
        solana_stake_program::stake_state,
    };

    #[test]
    fn test_generate_stake_meta_collection_happy_path() {
        /* 1. Create a Bank seeded with some validator stake accounts */
        let validator_keypairs_0 = ValidatorVoteKeypairs::new_rand();
        let validator_keypairs_1 = ValidatorVoteKeypairs::new_rand();
        let validator_keypairs_2 = ValidatorVoteKeypairs::new_rand();
        let validator_keypairs = vec![
            &validator_keypairs_0,
            &validator_keypairs_1,
            &validator_keypairs_2,
        ];
        const INITIAL_VALIDATOR_STAKES: u64 = 10_000;
        let GenesisConfigInfo { genesis_config, .. } = create_genesis_config_with_vote_accounts(
            1_000_000_000,
            &validator_keypairs,
            vec![INITIAL_VALIDATOR_STAKES; 3],
        );

        let (mut bank, _bank_forks) = Bank::new_with_bank_forks_for_tests(&genesis_config);

        /* 2. Seed the Bank with [RewardCollectionAccount]'s */
        let merkle_root_upload_authority = Pubkey::new_unique();
        let reward_distribution_program_id = Pubkey::new_unique();

        let delegator_0 = Keypair::new();
        let delegator_1 = Keypair::new();
        let delegator_2 = Keypair::new();
        let delegator_3 = Keypair::new();
        let delegator_4 = Keypair::new();

        let delegator_0_pk = delegator_0.pubkey();
        let delegator_1_pk = delegator_1.pubkey();
        let delegator_2_pk = delegator_2.pubkey();
        let delegator_3_pk = delegator_3.pubkey();
        let delegator_4_pk = delegator_4.pubkey();

        let d_0_data = AccountSharedData::new(
            300_000_000_000_000 * 10,
            0,
            &solana_sdk::system_program::id(),
        );
        let d_1_data = AccountSharedData::new(
            100_000_203_000_000 * 10,
            0,
            &solana_sdk::system_program::id(),
        );
        let d_2_data = AccountSharedData::new(
            100_000_235_899_000 * 10,
            0,
            &solana_sdk::system_program::id(),
        );
        let d_3_data = AccountSharedData::new(
            200_000_000_000_000 * 10,
            0,
            &solana_sdk::system_program::id(),
        );
        let d_4_data = AccountSharedData::new(
            100_000_000_777_000 * 10,
            0,
            &solana_sdk::system_program::id(),
        );

        bank.store_account(&delegator_0_pk, &d_0_data);
        bank.store_account(&delegator_1_pk, &d_1_data);
        bank.store_account(&delegator_2_pk, &d_2_data);
        bank.store_account(&delegator_3_pk, &d_3_data);
        bank.store_account(&delegator_4_pk, &d_4_data);

        /* 3. Delegate some stake to the initial set of validators */
        let mut validator_0_delegations = vec![crate::Delegation {
            stake_account_pubkey: validator_keypairs_0.stake_keypair.pubkey(),
            staker_pubkey: validator_keypairs_0.stake_keypair.pubkey(),
            withdrawer_pubkey: validator_keypairs_0.stake_keypair.pubkey(),
            lamports_delegated: INITIAL_VALIDATOR_STAKES,
        }];
        let stake_account = delegate_stake_helper(
            &bank,
            &delegator_0,
            &validator_keypairs_0.vote_keypair.pubkey(),
            30_000_000_000,
        );
        validator_0_delegations.push(crate::Delegation {
            stake_account_pubkey: stake_account,
            staker_pubkey: delegator_0.pubkey(),
            withdrawer_pubkey: delegator_0.pubkey(),
            lamports_delegated: 30_000_000_000,
        });
        let stake_account = delegate_stake_helper(
            &bank,
            &delegator_1,
            &validator_keypairs_0.vote_keypair.pubkey(),
            3_000_000_000,
        );
        validator_0_delegations.push(crate::Delegation {
            stake_account_pubkey: stake_account,
            staker_pubkey: delegator_1.pubkey(),
            withdrawer_pubkey: delegator_1.pubkey(),
            lamports_delegated: 3_000_000_000,
        });
        let stake_account = delegate_stake_helper(
            &bank,
            &delegator_2,
            &validator_keypairs_0.vote_keypair.pubkey(),
            33_000_000_000,
        );
        validator_0_delegations.push(crate::Delegation {
            stake_account_pubkey: stake_account,
            staker_pubkey: delegator_2.pubkey(),
            withdrawer_pubkey: delegator_2.pubkey(),
            lamports_delegated: 33_000_000_000,
        });

        let mut validator_1_delegations = vec![crate::Delegation {
            stake_account_pubkey: validator_keypairs_1.stake_keypair.pubkey(),
            staker_pubkey: validator_keypairs_1.stake_keypair.pubkey(),
            withdrawer_pubkey: validator_keypairs_1.stake_keypair.pubkey(),
            lamports_delegated: INITIAL_VALIDATOR_STAKES,
        }];
        let stake_account = delegate_stake_helper(
            &bank,
            &delegator_3,
            &validator_keypairs_1.vote_keypair.pubkey(),
            4_222_364_000,
        );
        validator_1_delegations.push(crate::Delegation {
            stake_account_pubkey: stake_account,
            staker_pubkey: delegator_3.pubkey(),
            withdrawer_pubkey: delegator_3.pubkey(),
            lamports_delegated: 4_222_364_000,
        });
        let stake_account = delegate_stake_helper(
            &bank,
            &delegator_4,
            &validator_keypairs_1.vote_keypair.pubkey(),
            6_000_000_527,
        );
        validator_1_delegations.push(crate::Delegation {
            stake_account_pubkey: stake_account,
            staker_pubkey: delegator_4.pubkey(),
            withdrawer_pubkey: delegator_4.pubkey(),
            lamports_delegated: 6_000_000_527,
        });

        let mut validator_2_delegations = vec![crate::Delegation {
            stake_account_pubkey: validator_keypairs_2.stake_keypair.pubkey(),
            staker_pubkey: validator_keypairs_2.stake_keypair.pubkey(),
            withdrawer_pubkey: validator_keypairs_2.stake_keypair.pubkey(),
            lamports_delegated: INITIAL_VALIDATOR_STAKES,
        }];
        let stake_account = delegate_stake_helper(
            &bank,
            &delegator_0,
            &validator_keypairs_2.vote_keypair.pubkey(),
            1_300_123_156,
        );
        validator_2_delegations.push(crate::Delegation {
            stake_account_pubkey: stake_account,
            staker_pubkey: delegator_0.pubkey(),
            withdrawer_pubkey: delegator_0.pubkey(),
            lamports_delegated: 1_300_123_156,
        });
        let stake_account = delegate_stake_helper(
            &bank,
            &delegator_4,
            &validator_keypairs_2.vote_keypair.pubkey(),
            1_610_565_420,
        );
        validator_2_delegations.push(crate::Delegation {
            stake_account_pubkey: stake_account,
            staker_pubkey: delegator_4.pubkey(),
            withdrawer_pubkey: delegator_4.pubkey(),
            lamports_delegated: 1_610_565_420,
        });

        /* 4. Run assertions */
        fn warmed_up(bank: &Bank, stake_pubkeys: &[Pubkey]) -> bool {
            for stake_pubkey in stake_pubkeys {
                let stake =
                    stake_state::stake_from(&bank.get_account(stake_pubkey).unwrap()).unwrap();

                if stake.delegation.stake
                    != stake.stake(
                        bank.epoch(),
                        &from_account::<StakeHistory, _>(
                            &bank.get_account(&sysvar::stake_history::id()).unwrap(),
                        )
                        .unwrap(),
                        bank.new_warmup_cooldown_rate_epoch(),
                    )
                {
                    return false;
                }
            }

            true
        }
        fn next_epoch(bank: &Arc<Bank>) -> Arc<Bank> {
            bank.squash();

            Arc::new(Bank::new_from_parent(
                bank.clone(),
                &Pubkey::default(),
                bank.get_slots_in_epoch(bank.epoch()) + bank.slot(),
            ))
        }

        let mut stake_pubkeys = validator_0_delegations
            .iter()
            .map(|v| v.stake_account_pubkey)
            .collect::<Vec<Pubkey>>();
        stake_pubkeys.extend(
            validator_1_delegations
                .iter()
                .map(|v| v.stake_account_pubkey),
        );
        stake_pubkeys.extend(
            validator_2_delegations
                .iter()
                .map(|v| v.stake_account_pubkey),
        );
        loop {
            if warmed_up(&bank, &stake_pubkeys[..]) {
                break;
            }

            // Cycle thru banks until we're fully warmed up
            bank = next_epoch(&bank);
        }

        let reward_distribution_account_0 = derive_reward_distribution_account_address(
            &reward_distribution_program_id,
            &validator_keypairs_0.vote_keypair.pubkey(),
            bank.epoch(),
        );
        let reward_distribution_account_1 = derive_reward_distribution_account_address(
            &reward_distribution_program_id,
            &validator_keypairs_1.vote_keypair.pubkey(),
            bank.epoch(),
        );
        let reward_distribution_account_2 = derive_reward_distribution_account_address(
            &reward_distribution_program_id,
            &validator_keypairs_2.vote_keypair.pubkey(),
            bank.epoch(),
        );

        let expires_at = bank.epoch() + 3;

        let rda_0 = RewardCollectionAccount {
            validator_vote_account: validator_keypairs_0.vote_keypair.pubkey(),
            merkle_root_upload_authority,
            merkle_root: None,
            epoch_created_at: bank.epoch(),
            validator_commission_bps: 50,
            expires_at,
            bump: reward_distribution_account_0.1,
        };
        let rda_1 = RewardCollectionAccount {
            validator_vote_account: validator_keypairs_1.vote_keypair.pubkey(),
            merkle_root_upload_authority,
            merkle_root: None,
            epoch_created_at: bank.epoch(),
            validator_commission_bps: 500,
            expires_at: 0,
            bump: reward_distribution_account_1.1,
        };
        let rda_2 = RewardCollectionAccount {
            validator_vote_account: validator_keypairs_2.vote_keypair.pubkey(),
            merkle_root_upload_authority,
            merkle_root: None,
            epoch_created_at: bank.epoch(),
            validator_commission_bps: 75,
            expires_at: 0,
            bump: reward_distribution_account_2.1,
        };

        let reward_distro_0_rewards = 1_000_000 * 10;
        let reward_distro_1_rewards = 69_000_420 * 10;
        let reward_distro_2_rewards = 789_000_111 * 10;

        let rda_0_fields = (
            reward_distribution_account_0.0,
            rda_0.validator_commission_bps,
        );
        let data_0 = rda_to_account_shared_data(
            &reward_distribution_program_id,
            reward_distro_0_rewards,
            rda_0,
        );
        let rda_1_fields = (
            reward_distribution_account_1.0,
            rda_1.validator_commission_bps,
        );
        let data_1 = rda_to_account_shared_data(
            &reward_distribution_program_id,
            reward_distro_1_rewards,
            rda_1,
        );
        let rda_2_fields = (
            reward_distribution_account_2.0,
            rda_2.validator_commission_bps,
        );
        let data_2 = rda_to_account_shared_data(
            &reward_distribution_program_id,
            reward_distro_2_rewards,
            rda_2,
        );

        bank.store_account(&reward_distribution_account_0.0, &data_0);
        bank.store_account(&reward_distribution_account_1.0, &data_1);
        bank.store_account(&reward_distribution_account_2.0, &data_2);

        bank.freeze();
        let stake_meta_collection =
            generate_stake_meta_collection(&bank, &reward_distribution_program_id).unwrap();
        assert_eq!(
            stake_meta_collection.reward_distribution_program_id,
            reward_distribution_program_id
        );
        assert_eq!(stake_meta_collection.slot, bank.slot());
        assert_eq!(stake_meta_collection.epoch, bank.epoch());

        let mut expected_stake_metas = HashMap::new();
        expected_stake_metas.insert(
            validator_keypairs_0.vote_keypair.pubkey(),
            StakeMeta {
                validator_vote_account: validator_keypairs_0.vote_keypair.pubkey(),
                delegations: validator_0_delegations.clone(),
                total_delegated: validator_0_delegations
                    .iter()
                    .fold(0u64, |sum, delegation| {
                        sum.checked_add(delegation.lamports_delegated).unwrap()
                    }),
                maybe_reward_distribution_meta: Some(RewardDistributionMeta {
                    merkle_root_upload_authority,
                    reward_distribution_pubkey: rda_0_fields.0,
                    total_rewards: reward_distro_0_rewards
                        .checked_sub(
                            bank.get_minimum_balance_for_rent_exemption(
                                RewardCollectionAccount::SIZE,
                            ),
                        )
                        .unwrap(),
                    validator_fee_bps: rda_0_fields.1,
                }),
                commission: 0,
                validator_node_pubkey: validator_keypairs_0.node_keypair.pubkey(),
            },
        );
        expected_stake_metas.insert(
            validator_keypairs_1.vote_keypair.pubkey(),
            StakeMeta {
                validator_vote_account: validator_keypairs_1.vote_keypair.pubkey(),
                delegations: validator_1_delegations.clone(),
                total_delegated: validator_1_delegations
                    .iter()
                    .fold(0u64, |sum, delegation| {
                        sum.checked_add(delegation.lamports_delegated).unwrap()
                    }),
                maybe_reward_distribution_meta: Some(RewardDistributionMeta {
                    merkle_root_upload_authority,
                    reward_distribution_pubkey: rda_1_fields.0,
                    total_rewards: reward_distro_1_rewards
                        .checked_sub(
                            bank.get_minimum_balance_for_rent_exemption(
                                RewardCollectionAccount::SIZE,
                            ),
                        )
                        .unwrap(),
                    validator_fee_bps: rda_1_fields.1,
                }),
                commission: 0,
                validator_node_pubkey: validator_keypairs_1.node_keypair.pubkey(),
            },
        );
        expected_stake_metas.insert(
            validator_keypairs_2.vote_keypair.pubkey(),
            StakeMeta {
                validator_vote_account: validator_keypairs_2.vote_keypair.pubkey(),
                delegations: validator_2_delegations.clone(),
                total_delegated: validator_2_delegations
                    .iter()
                    .fold(0u64, |sum, delegation| {
                        sum.checked_add(delegation.lamports_delegated).unwrap()
                    }),
                maybe_reward_distribution_meta: Some(RewardDistributionMeta {
                    merkle_root_upload_authority,
                    reward_distribution_pubkey: rda_2_fields.0,
                    total_rewards: reward_distro_2_rewards
                        .checked_sub(
                            bank.get_minimum_balance_for_rent_exemption(
                                RewardCollectionAccount::SIZE,
                            ),
                        )
                        .unwrap(),
                    validator_fee_bps: rda_2_fields.1,
                }),
                commission: 0,
                validator_node_pubkey: validator_keypairs_2.node_keypair.pubkey(),
            },
        );

        println!(
            "validator_0 [vote_account={}, stake_account={}]",
            validator_keypairs_0.vote_keypair.pubkey(),
            validator_keypairs_0.stake_keypair.pubkey()
        );
        println!(
            "validator_1 [vote_account={}, stake_account={}]",
            validator_keypairs_1.vote_keypair.pubkey(),
            validator_keypairs_1.stake_keypair.pubkey()
        );
        println!(
            "validator_2 [vote_account={}, stake_account={}]",
            validator_keypairs_2.vote_keypair.pubkey(),
            validator_keypairs_2.stake_keypair.pubkey(),
        );

        assert_eq!(
            expected_stake_metas.len(),
            stake_meta_collection.stake_metas.len()
        );

        for actual_stake_meta in stake_meta_collection.stake_metas {
            let expected_stake_meta = expected_stake_metas
                .get(&actual_stake_meta.validator_vote_account)
                .unwrap();
            assert_eq!(
                expected_stake_meta.maybe_reward_distribution_meta,
                actual_stake_meta.maybe_reward_distribution_meta
            );
            assert_eq!(
                expected_stake_meta.total_delegated,
                actual_stake_meta.total_delegated
            );
            assert_eq!(expected_stake_meta.commission, actual_stake_meta.commission);
            assert_eq!(
                expected_stake_meta.validator_vote_account,
                actual_stake_meta.validator_vote_account
            );

            assert_eq!(
                expected_stake_meta.delegations.len(),
                actual_stake_meta.delegations.len()
            );

            for expected_delegation in &expected_stake_meta.delegations {
                let actual_delegation = actual_stake_meta
                    .delegations
                    .iter()
                    .find(|d| d.stake_account_pubkey == expected_delegation.stake_account_pubkey)
                    .unwrap();

                assert_eq!(expected_delegation, actual_delegation);
            }
        }
    }

    /// Helper function that sends a delegate stake instruction to the bank.
    /// Returns the created stake account pubkey.
    fn delegate_stake_helper(
        bank: &Bank,
        from_keypair: &Keypair,
        vote_account: &Pubkey,
        delegation_amount: u64,
    ) -> Pubkey {
        let minimum_delegation = solana_stake_program::get_minimum_delegation(&bank.feature_set);
        assert!(
            delegation_amount >= minimum_delegation,
            "{}",
            format!(
                "received delegation_amount {}, must be at least {}",
                delegation_amount, minimum_delegation
            )
        );
        if let Some(from_account) = bank.get_account(&from_keypair.pubkey()) {
            assert_eq!(from_account.owner(), &solana_sdk::system_program::id());
        } else {
            panic!("from_account DNE");
        }
        assert!(bank.get_account(vote_account).is_some());

        let stake_keypair = Keypair::new();
        let instructions = stake::instruction::create_account_and_delegate_stake(
            &from_keypair.pubkey(),
            &stake_keypair.pubkey(),
            vote_account,
            &Authorized::auto(&from_keypair.pubkey()),
            &Lockup::default(),
            delegation_amount,
        );

        let message = Message::new(&instructions[..], Some(&from_keypair.pubkey()));
        let transaction = Transaction::new(
            &[from_keypair, &stake_keypair],
            message,
            bank.last_blockhash(),
        );

        bank.process_transaction(&transaction)
            .map_err(|e| {
                eprintln!("Error delegating stake [error={}]", e);
                e
            })
            .unwrap();

        stake_keypair.pubkey()
    }

    fn rda_to_account_shared_data(
        reward_distribution_program_id: &Pubkey,
        lamports: u64,
        rda: RewardCollectionAccount,
    ) -> AccountSharedData {
        let mut account_data = AccountSharedData::new(
            lamports,
            RewardCollectionAccount::SIZE,
            reward_distribution_program_id,
        );

        let mut data: [u8; RewardCollectionAccount::SIZE] = [0u8; RewardCollectionAccount::SIZE];
        let mut cursor = std::io::Cursor::new(&mut data[..]);
        rda.try_serialize(&mut cursor).unwrap();

        account_data.set_data(data.to_vec());
        account_data
    }
}
