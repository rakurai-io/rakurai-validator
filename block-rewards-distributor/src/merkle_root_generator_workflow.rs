use {
    crate::{
        distribution_config_parser::DistributionConfig, read_json_from_file,
        GeneratedMerkleTreeCollection, StakeMetaCollection,
    },
    log::*,
    solana_client::rpc_client::RpcClient,
    solana_metrics::datapoint_info,
    std::{
        fmt::Debug,
        fs::File,
        io::{BufWriter, Write},
        path::PathBuf,
        thread,
        time::Duration,
    },
    thiserror::Error,
};

#[derive(Error, Debug)]
pub enum MerkleRootGeneratorError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    RpcError(#[from] Box<solana_client::client_error::ClientError>),

    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),

    #[error("Math overflow occurred")]
    MathOverflow,
}

pub fn write_to_db(merkle_tree_coll: &GeneratedMerkleTreeCollection) {
    let mut rakurai_delegated_stake = 0;
    let mut total_block_rewards = 0;
    for merkle_tree in &merkle_tree_coll.generated_merkle_trees {
        rakurai_delegated_stake += merkle_tree.total_delegated;
        total_block_rewards += merkle_tree.max_total_claim;
        datapoint_info!(
            "validators_info",
            ("epoch", merkle_tree_coll.epoch, i64),
            (
                "vote_account",
                &merkle_tree.validator_vote_account.to_string(),
                String
            ),
            (
                "identity",
                &merkle_tree.validator_node_pubkey.to_string(),
                String
            ),
            (
                "reward_distribution_account",
                &merkle_tree.reward_distribution_account.to_string(),
                String
            ),
            (
                "merkle_root_upload_authority",
                &merkle_tree.merkle_root_upload_authority.to_string(),
                String
            ),
            ("total_delegated_stake", merkle_tree.total_delegated, i64),
            ("voting_commission", merkle_tree.voting_commission, i64),
            (
                "rakurai_commission_bps",
                merkle_tree.rakurai_commission_bps as i64,
                i64
            ),
            (
                "validator_commission_bps",
                merkle_tree.validator_commission_bps as i64,
                i64
            ),
            ("total_rewards", merkle_tree.max_total_claim as i64, i64)
        );

        for node in &merkle_tree.tree_nodes {
            datapoint_info!(
                "stakers_info",
                ("epoch", merkle_tree_coll.epoch, i64),
                (
                    "vote_account",
                    &merkle_tree.validator_vote_account.to_string(),
                    String
                ),
                (
                    "identity",
                    &merkle_tree.validator_node_pubkey.to_string(),
                    String
                ),
                (
                    "stake_pubkey",
                    &node.stake_account_pubkey.to_string(),
                    String
                ),
                ("staker", &node.staker_pubkey.to_string(), String),
                ("withdrawer", &node.withdrawer_pubkey.to_string(), String),
                ("active_stake", node.active_stake as i64, i64),
                ("expected_rewards", node.amount as i64, i64)
            );
        }
    }

    let reward_per_lamport = total_block_rewards as f64 / rakurai_delegated_stake as f64;

    datapoint_info!(
        "cluster_stats",
        ("epoch", merkle_tree_coll.epoch, i64),
        ("rakurai_delegated_stake", rakurai_delegated_stake, i64),
        ("total_block_rewards", total_block_rewards, i64),
        ("reward_per_lamport", reward_per_lamport, f64),
    );

    //to ensure that that the data should be written to db before application stop.
    thread::sleep(Duration::from_secs(12));
}

pub fn generate_merkle_root(
    distribution_config_path: &PathBuf,
    stake_meta_coll_path: &PathBuf,
    out_path: &PathBuf,
    rpc_url: &str,
) -> Result<(), MerkleRootGeneratorError> {
    let stake_meta_coll: StakeMetaCollection = read_json_from_file(stake_meta_coll_path)?;
    let mut distribution_config: DistributionConfig =
        read_json_from_file(distribution_config_path).unwrap();
    datapoint_info!(
        "snapshot_info",
        ("epoch", stake_meta_coll.epoch, i64),
        ("bank_slot", stake_meta_coll.slot, i64),
        ("bank_hash", stake_meta_coll.bank_hash.to_string(), String),
        (
            "reward_distribution_program_id",
            stake_meta_coll.reward_distribution_program_id.to_string(),
            String
        )
    );

    let rpc_client = RpcClient::new(rpc_url);
    let merkle_tree_coll =
        GeneratedMerkleTreeCollection::new_from_stake_meta_collection_with_config(
            stake_meta_coll,
            Some(rpc_client),
            &mut distribution_config,
        )?;

    write_to_db(&merkle_tree_coll);

    write_to_json_file(&merkle_tree_coll, out_path)?;
    Ok(())
}

fn write_to_json_file(
    merkle_tree_coll: &GeneratedMerkleTreeCollection,
    file_path: &PathBuf,
) -> Result<(), MerkleRootGeneratorError> {
    let file = File::create(file_path)?;
    let mut writer = BufWriter::new(file);
    let json = serde_json::to_string_pretty(&merkle_tree_coll).unwrap();
    writer.write_all(json.as_bytes())?;
    writer.flush()?;

    Ok(())
}
