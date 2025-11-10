use {
    crate::{
        read_json_from_file, sign_and_send_transactions_with_retries, GeneratedMerkleTree,
        GeneratedMerkleTreeCollection,
    },
    anchor_lang::{prelude::Pubkey as AnchorPubkey, AccountDeserialize},
    log::{error, info},
    reward_distribution::{
        sdk::instruction::{upload_merkle_root_ix, UploadMerkleRootAccounts, UploadMerkleRootArgs},
        state::{RewardCollectionAccount, RewardDistributionConfigAccount},
    },
    solana_client::nonblocking::rpc_client::RpcClient,
    solana_commitment_config::CommitmentConfig,
    solana_program::{
        fee_calculator::DEFAULT_TARGET_LAMPORTS_PER_SIGNATURE, native_token::LAMPORTS_PER_SOL,
    },
    solana_sdk::{
        instruction::{AccountMeta, Instruction},
        pubkey::Pubkey,
        signature::{read_keypair_file, Signer},
        transaction::Transaction,
    },
    std::{path::PathBuf, time::Duration},
    thiserror::Error,
    tokio::runtime::Builder,
};

#[derive(Error, Debug)]
pub enum MerkleRootUploadError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    JsonError(#[from] serde_json::Error),
}

pub fn upload_merkle_root(
    merkle_root_path: &PathBuf,
    keypair_path: &PathBuf,
    rpc_url: &str,
    reward_distribution_program_id: &Pubkey,
    max_concurrent_rpc_get_reqs: usize,
    txn_send_batch_size: usize,
) -> Result<(), MerkleRootUploadError> {
    const MAX_RETRY_DURATION: Duration = Duration::from_secs(600);

    let merkle_tree: GeneratedMerkleTreeCollection =
        read_json_from_file(merkle_root_path).expect("read GeneratedMerkleTreeCollection");
    let keypair = read_keypair_file(keypair_path).expect("read keypair file");

    let reward_distribution_config = Pubkey::find_program_address(
        &[RewardDistributionConfigAccount::SEED],
        reward_distribution_program_id,
    )
    .0;

    let runtime = Builder::new_multi_thread()
        .worker_threads(16)
        .enable_all()
        .build()
        .expect("build runtime");

    runtime.block_on(async move {
        let rpc_client =
            RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());
        let trees: Vec<GeneratedMerkleTree> = merkle_tree
            .generated_merkle_trees
            .into_iter()
            .filter(|tree| tree.merkle_root_upload_authority == keypair.pubkey())
            .collect();

        info!("num trees to upload: {:?}", trees.len());

        // heuristic to make sure we have enough funds to cover execution, assumes all trees need updating
        {
            let initial_balance = rpc_client.get_balance(&keypair.pubkey()).await.expect("failed to get balance");
            let desired_balance = (trees.len() as u64).checked_mul(DEFAULT_TARGET_LAMPORTS_PER_SIGNATURE).unwrap();
            if initial_balance < desired_balance {
                let sol_to_deposit = desired_balance.checked_sub(initial_balance).unwrap().checked_add(LAMPORTS_PER_SOL).unwrap().checked_sub(1).unwrap().checked_div(LAMPORTS_PER_SOL).unwrap(); // rounds up to nearest sol
                panic!("Expected to have at least {} lamports in {}, current balance is {} lamports, deposit {} SOL to continue.",
                       desired_balance, &keypair.pubkey(), initial_balance, sol_to_deposit)
            }
        }
        let mut trees_needing_update: Vec<GeneratedMerkleTree> = vec![];
        for tree in trees {
            let account = rpc_client
                .get_account(&tree.reward_distribution_account)
                .await
                .expect("fetch expect");

            let mut data = account.data.as_slice();
            let fetched_reward_distribution_account =
            RewardCollectionAccount::try_deserialize(&mut data)
                    .expect("failed to deserialize reward_distribution_account state");

            let needs_upload = match fetched_reward_distribution_account.merkle_root {
                Some(merkle_root) => {
                    merkle_root.total_funds_claimed == 0
                        && merkle_root.root != tree.merkle_root.to_bytes()
                }
                None => true,
            };

            if needs_upload {
                trees_needing_update.push(tree);
            }
        }

        info!("num trees need uploading: {:?}", trees_needing_update.len());

        let transactions: Vec<Transaction> = trees_needing_update
            .iter()
            .map(|tree| {
                let mut ix = upload_merkle_root_ix(
                    AnchorPubkey::from(reward_distribution_program_id.as_array().clone()),
                    UploadMerkleRootArgs {
                        root: tree.merkle_root.to_bytes(),
                        max_total_claim: tree.max_total_claim,
                        max_num_nodes: tree.max_num_nodes,
                    },
                    UploadMerkleRootAccounts {
                        config: AnchorPubkey::from(reward_distribution_config.as_array().clone()),
                        merkle_root_upload_authority: AnchorPubkey::from(keypair.pubkey().as_array().clone()),
                        reward_collection_account: AnchorPubkey::from(tree.reward_distribution_account.as_array().clone()),
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

            let instruction = Instruction::new_with_bytes(
                *reward_distribution_program_id,
                &ix.data,
                acct_metas,
            );

                Transaction::new_with_payer(
                    &[instruction],
                    Some(&keypair.pubkey()),
                )
            })
            .collect();

        let (to_process, failed_transactions) = sign_and_send_transactions_with_retries(
            &keypair, &rpc_client, max_concurrent_rpc_get_reqs, transactions, txn_send_batch_size, MAX_RETRY_DURATION).await;
        if !to_process.is_empty() {
            panic!("{} remaining claim transactions, {} failed requests.", to_process.len(), failed_transactions.len());
        }
    });

    Ok(())
}
