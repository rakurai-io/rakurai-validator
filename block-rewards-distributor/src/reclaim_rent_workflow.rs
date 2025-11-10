use {
    crate::{
        claim_workflow::ClaimRewardsError, get_batched_accounts, send_until_blockhash_expires,
    },
    anchor_lang::{prelude::Pubkey as AnchorPubkey, AccountDeserialize},
    log::{info, warn},
    rand::{prelude::SliceRandom, thread_rng},
    reward_distribution::{
        sdk::{
            derive_config_account_address,
            instruction::{
                close_claim_status_ix, close_reward_collection_account_ix,
                CloseClaimStatusAccounts, CloseClaimStatusArgs, CloseRewardCollectionAccountArgs,
                CloseRewardCollectionAccounts,
            },
        },
        state::{ClaimStatus, RewardCollectionAccount},
    },
    solana_client::{nonblocking::rpc_client::RpcClient, rpc_client::SerializableTransaction},
    solana_commitment_config::CommitmentConfig,
    solana_compute_budget_interface::ComputeBudgetInstruction,
    solana_program::{clock::Epoch, pubkey::Pubkey},
    solana_rpc_client_api::config::RpcSimulateTransactionConfig,
    solana_sdk::{
        account::Account,
        instruction::{AccountMeta, Instruction},
        signature::{Keypair, Signature, Signer},
        transaction::Transaction,
    },
    std::{
        collections::HashMap,
        sync::Arc,
        time::{Duration, Instant},
    },
};

pub const CLOSE_INSTR_PER_TXN: usize = 4;

/// Clear old ClaimStatus accounts
pub async fn reclaim_rent(
    rpc_url: String,
    reward_distribution_program_id: Pubkey,
    signer: Arc<Keypair>,
    max_loop_duration: Duration,
    // Optionally reclaim RewardCollectionAccount rents on behalf of validators.
    should_reclaim_rdas: bool,
    micro_lamports: u64,
) -> Result<(), ClaimRewardsError> {
    let rpc_client = RpcClient::new_with_timeout_and_commitment(
        rpc_url.clone(),
        Duration::from_secs(300),
        CommitmentConfig::processed(),
    );

    let start = Instant::now();

    let accounts = rpc_client
        .get_program_accounts(&reward_distribution_program_id)
        .await?;

    let config_pubkey = derive_config_account_address(&AnchorPubkey::from(
        reward_distribution_program_id.as_array().clone(),
    ))
    .0;

    let epoch = rpc_client.get_epoch_info().await?.epoch;
    let mut claim_status_pubkeys_to_expire =
        find_expired_claim_status_accounts(&accounts, epoch, signer.pubkey());
    let mut rda_pubkeys_to_expire = find_expired_rda_accounts(&accounts, epoch);

    while start.elapsed() <= max_loop_duration {
        let mut transactions = build_close_claim_status_transactions(
            &claim_status_pubkeys_to_expire,
            reward_distribution_program_id,
            Pubkey::from(config_pubkey.as_array().clone()),
            micro_lamports,
            signer.pubkey(),
        );
        if should_reclaim_rdas {
            transactions.extend(build_close_rda_transactions(
                &rda_pubkeys_to_expire,
                reward_distribution_program_id,
                Pubkey::from(config_pubkey.as_array().clone()),
                signer.pubkey(),
            ));
        }

        if transactions.is_empty() {
            info!("Finished reclaim rent!");
            return Ok(());
        }

        transactions.shuffle(&mut thread_rng());
        let transactions: Vec<_> = transactions.into_iter().take(10_000).collect();
        let blockhash = rpc_client.get_latest_blockhash().await?;
        let txns_to_claim: HashMap<Signature, Transaction /*Struct */> = transactions
            .into_iter()
            .map(|mut tx| {
                tx.sign(&[&signer], blockhash);
                (*tx.get_signature(), tx)
            })
            .collect();

        let claim_transactions: Vec<(Signature, Transaction)> =
            txns_to_claim.clone().into_iter().collect();

        send_until_blockhash_expires(&rpc_client, claim_transactions, blockhash).await?;

        // can just refresh calling get_multiple_accounts since these operations should be subtractive and not additive
        let claim_status_pubkeys: Vec<_> = claim_status_pubkeys_to_expire
            .iter()
            .map(|(pubkey, _)| *pubkey)
            .collect();
        claim_status_pubkeys_to_expire = get_batched_accounts(&rpc_client, &claim_status_pubkeys)
            .await?
            .into_iter()
            .filter_map(|(pubkey, account)| Some((pubkey, account?)))
            .collect();

        let rda_pubkeys: Vec<_> = rda_pubkeys_to_expire
            .iter()
            .map(|(pubkey, _)| *pubkey)
            .collect();
        rda_pubkeys_to_expire = get_batched_accounts(&rpc_client, &rda_pubkeys)
            .await?
            .into_iter()
            .filter_map(|(pubkey, account)| Some((pubkey, account?)))
            .collect();
    }

    // one final refresh before double checking everything
    let claim_status_pubkeys: Vec<_> = claim_status_pubkeys_to_expire
        .iter()
        .map(|(pubkey, _)| *pubkey)
        .collect();
    claim_status_pubkeys_to_expire = get_batched_accounts(&rpc_client, &claim_status_pubkeys)
        .await?
        .into_iter()
        .filter_map(|(pubkey, account)| Some((pubkey, account?)))
        .collect();

    let rda_pubkeys: Vec<_> = rda_pubkeys_to_expire
        .iter()
        .map(|(pubkey, _)| *pubkey)
        .collect();
    rda_pubkeys_to_expire = get_batched_accounts(&rpc_client, &rda_pubkeys)
        .await?
        .into_iter()
        .filter_map(|(pubkey, account)| Some((pubkey, account?)))
        .collect();

    let mut transactions = build_close_claim_status_transactions(
        &claim_status_pubkeys_to_expire,
        reward_distribution_program_id,
        Pubkey::from(config_pubkey.as_array().clone()),
        micro_lamports,
        signer.pubkey(),
    );
    if should_reclaim_rdas {
        transactions.extend(build_close_rda_transactions(
            &rda_pubkeys_to_expire,
            reward_distribution_program_id,
            Pubkey::from(config_pubkey.as_array().clone()),
            signer.pubkey(),
        ));
    }

    if transactions.is_empty() {
        return Ok(());
    }

    // if more transactions left, we'll simulate them all to make sure its not an uncaught error
    let mut is_error = false;
    let mut error_str = String::new();
    for tx in &transactions {
        match rpc_client
            .simulate_transaction_with_config(
                tx,
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
                        warn!("transaction error. tx: {:?} error: {:?}", tx, e);
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

fn find_expired_claim_status_accounts(
    accounts: &[(Pubkey, Account)],
    epoch: Epoch,
    payer: Pubkey,
) -> Vec<(Pubkey, Account)> {
    accounts
        .iter()
        .filter_map(|(pubkey, account)| {
            let claim_status = ClaimStatus::try_deserialize(&mut account.data.as_slice()).ok()?;
            if claim_status
                .claim_status_payer
                .eq(&AnchorPubkey::from(payer.as_array().clone()))
                && epoch > claim_status.expires_at
            {
                Some((*pubkey, account.clone()))
            } else {
                None
            }
        })
        .collect()
}

fn find_expired_rda_accounts(
    accounts: &[(Pubkey, Account)],
    epoch: Epoch,
) -> Vec<(Pubkey, Account)> {
    accounts
        .iter()
        .filter_map(|(pubkey, account)| {
            let rda =
                RewardCollectionAccount::try_deserialize(&mut account.data.as_slice()).ok()?;
            if epoch > rda.expires_at {
                Some((*pubkey, account.clone()))
            } else {
                None
            }
        })
        .collect()
}

/// Assumes accounts is already pre-filtered with checks to ensure the account can be closed
fn build_close_claim_status_transactions(
    accounts: &[(Pubkey, Account)],
    reward_distribution_program_id: Pubkey,
    config: Pubkey,
    microlamports: u64,
    payer: Pubkey,
) -> Vec<Transaction> {
    accounts
        .iter()
        .map(|(claim_status_pubkey, account)| {
            let claim_status = ClaimStatus::try_deserialize(&mut account.data.as_slice()).unwrap();
            close_claim_status_ix(
                AnchorPubkey::from(reward_distribution_program_id.as_array().clone()),
                CloseClaimStatusArgs,
                CloseClaimStatusAccounts {
                    config: AnchorPubkey::from(config.as_array().clone()),
                    claim_status: AnchorPubkey::from(claim_status_pubkey.as_array().clone()),
                    claim_status_payer: AnchorPubkey::from(
                        claim_status.claim_status_payer.as_array().clone(),
                    ),
                },
            )
        })
        .collect::<Vec<_>>()
        .chunks(CLOSE_INSTR_PER_TXN)
        .map(|close_claim_status_instructions| {
            let mut close_claim_status_instructions =
                close_claim_status_instructions.first().unwrap().clone();
            let acct_metas: Vec<AccountMeta> = close_claim_status_instructions
                .accounts
                .iter_mut()
                .map(|acct| AccountMeta {
                    pubkey: Pubkey::from(acct.pubkey.as_array().clone()),
                    is_signer: acct.is_signer,
                    is_writable: acct.is_writable,
                })
                .collect();

            let close_claim_status_instructions = vec![Instruction::new_with_bytes(
                reward_distribution_program_id,
                &close_claim_status_instructions.data,
                acct_metas,
            )];

            let mut instructions = vec![ComputeBudgetInstruction::set_compute_unit_price(
                microlamports,
            )];
            instructions.extend(close_claim_status_instructions);
            Transaction::new_with_payer(&instructions, Some(&payer))
        })
        .collect()
}

fn build_close_rda_transactions(
    accounts: &[(Pubkey, Account)],
    reward_distribution_program_id: Pubkey,
    config_pubkey: Pubkey,
    payer: Pubkey,
) -> Vec<Transaction> {
    let instructions: Vec<_> = accounts
        .iter()
        .map(|(pubkey, account)| {
            let rda =
                RewardCollectionAccount::try_deserialize(&mut account.data.as_slice()).unwrap();
            let mut instruction = close_reward_collection_account_ix(
                AnchorPubkey::from(reward_distribution_program_id.as_array().clone()),
                CloseRewardCollectionAccountArgs {
                    _epoch: rda.creation_epoch,
                },
                CloseRewardCollectionAccounts {
                    config: AnchorPubkey::from(config_pubkey.as_array().clone()),
                    reward_collection_account: AnchorPubkey::from(pubkey.as_array().clone()),
                    validator_vote_account: rda.validator_vote_account,
                    initializer: rda.initializer,
                    signer: AnchorPubkey::from(payer.as_array().clone()),
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

            let instruction = Instruction::new_with_bytes(
                reward_distribution_program_id,
                &instruction.data,
                acct_metas,
            );

            instruction
        })
        .collect();

    instructions
        .chunks(CLOSE_INSTR_PER_TXN)
        .map(|ix_chunk| Transaction::new_with_payer(ix_chunk, Some(&payer)))
        .collect()
}
