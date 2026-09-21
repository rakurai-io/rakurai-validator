//! This binary generates a merkle tree for each [RewardDistributionAccount]; they are derived
//! using a user provided [StakeMetaCollection] JSON file.

use {
    block_rewards_distributor::merkle_root_generator_workflow::generate_merkle_root, clap::Parser,
    log::*, std::path::PathBuf,
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to JSON file containing the [StakeMetaCollection] object.
    #[arg(long, env)]
    stake_meta_coll_path: PathBuf,

    /// Path to Json file  containing the [DistributionConfig] object.
    #[arg(long, env)]
    distribution_config_path: PathBuf,

    /// RPC to send transactions through. Used to validate what's being claimed is equal to RDA balance minus rent.
    #[arg(long, env)]
    rpc_url: String,

    /// Path to JSON file to get populated with tree node data.
    #[arg(long, env)]
    out_path: PathBuf,
}

fn main() {
    env_logger::init();
    info!("Starting merkle-root-generator workflow...");

    let args: Args = Args::parse();
    generate_merkle_root(
        &args.distribution_config_path,
        &args.stake_meta_coll_path,
        &args.out_path,
        &args.rpc_url,
    )
    .expect("merkle tree produced");
    info!("saved merkle roots to {:?}", args.stake_meta_coll_path);
}
