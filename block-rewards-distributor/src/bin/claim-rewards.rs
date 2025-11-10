use {
    block_rewards_distributor::{
        claim_workflow::{claim_rewards, ClaimRewardsError},
        read_json_from_file,
        reclaim_rent_workflow::reclaim_rent,
        GeneratedMerkleTreeCollection,
    },
    clap::Parser,
    futures::future::join_all,
    gethostname::gethostname,
    log::*,
    solana_metrics::set_host_id,
    solana_sdk::{
        pubkey::Pubkey,
        signature::{read_keypair_file, Keypair},
    },
    std::{
        path::PathBuf,
        sync::Arc,
        time::{Duration, Instant},
    },
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to JSON file containing the [GeneratedMerkleTreeCollection] object.
    #[arg(long, env)]
    merkle_trees_path: PathBuf,

    /// RPC to send transactions through
    #[arg(long, env, default_value = "http://localhost:8899")]
    rpc_url: String,

    /// rewards distribution program ID
    #[arg(long, env)]
    reward_distribution_program_id: Pubkey,

    /// Path to keypair
    #[arg(long, env)]
    keypair_path: PathBuf,

    /// Limits how long before send loop runs before stopping
    #[arg(long, env, default_value_t = 60 * 60)]
    max_retry_duration_secs: u64,

    /// Specifies whether to reclaim any rent.
    #[arg(long, env, default_value_t = true)]
    should_reclaim_rent: bool,

    /// Specifies whether to reclaim rent on behalf of validators from respective RDAs.
    #[arg(long, env)]
    should_reclaim_rdas: bool,

    /// The price to pay for priority fee
    #[arg(long, env, default_value_t = 1)]
    micro_lamports: u64,
}

async fn start_claim_process(
    merkle_trees: GeneratedMerkleTreeCollection,
    rpc_url: String,
    reward_distribution_program_id: Pubkey,
    signer: Arc<Keypair>,
    max_loop_duration: Duration,
    micro_lamports: u64,
) -> Result<(), ClaimRewardsError> {
    let start = Instant::now();

    match claim_rewards(
        &merkle_trees,
        rpc_url,
        reward_distribution_program_id,
        signer,
        max_loop_duration,
        micro_lamports,
    )
    .await
    {
        Err(e) => {
            error!(
                "claim_workflow_error: epoch={}, err_str={}, elapsed_us={}",
                merkle_trees.epoch,
                e.to_string(),
                start.elapsed().as_micros()
            );
            Err(e)
        }
        Ok(()) => {
            info!(
                "claim_workflow_completion: epoch={}, elapsed_us={}",
                merkle_trees.epoch,
                start.elapsed().as_micros()
            );
            Ok(())
        }
    }
}

async fn start_rent_claim(
    rpc_url: String,
    reward_distribution_program_id: Pubkey,
    signer: Arc<Keypair>,
    max_loop_duration: Duration,
    should_reclaim_rdas: bool,
    micro_lamports: u64,
    epoch: u64,
) -> Result<(), ClaimRewardsError> {
    let start = Instant::now();
    match reclaim_rent(
        rpc_url,
        reward_distribution_program_id,
        signer,
        max_loop_duration,
        should_reclaim_rdas,
        micro_lamports,
    )
    .await
    {
        Err(e) => {
            error!(
                "claim_workflow_reclaim_rent_error for epoch= {}, err_str= {} and elapsed_us= {} ",
                epoch,
                e.to_string(),
                start.elapsed().as_micros()
            );
            Err(e)
        }
        Ok(()) => {
            info!(
                "claim_workflow_reclaim_rent_completion: epoch={}, elapsed_us={}",
                epoch,
                start.elapsed().as_micros()
            );
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), ClaimRewardsError> {
    env_logger::init();

    gethostname()
        .into_string()
        .map(set_host_id)
        .expect("set hostname");

    let args: Args = Args::parse();
    let keypair = Arc::new(read_keypair_file(&args.keypair_path).expect("read keypair file"));
    let merkle_trees: GeneratedMerkleTreeCollection =
        read_json_from_file(&args.merkle_trees_path).expect("read GeneratedMerkleTreeCollection");
    let max_loop_duration = Duration::from_secs(args.max_retry_duration_secs);

    info!(
        "Starting to claim rewards for epoch: {}",
        merkle_trees.epoch
    );
    let epoch = merkle_trees.epoch;

    let mut futs = vec![];
    futs.push(tokio::spawn(start_claim_process(
        merkle_trees,
        args.rpc_url.clone(),
        args.reward_distribution_program_id,
        keypair.clone(),
        max_loop_duration,
        args.micro_lamports,
    )));
    if args.should_reclaim_rent {
        futs.push(tokio::spawn(start_rent_claim(
            args.rpc_url.clone(),
            args.reward_distribution_program_id,
            keypair.clone(),
            max_loop_duration,
            args.should_reclaim_rdas,
            args.micro_lamports,
            epoch,
        )));
    }
    let results = join_all(futs).await;
    solana_metrics::flush(); // sometimes last datapoint doesn't get emitted. this increases likelihood.
    for r in results {
        r.map_err(|e| ClaimRewardsError::UncaughtError { e: e.to_string() })??;
    }
    Ok(())
}
