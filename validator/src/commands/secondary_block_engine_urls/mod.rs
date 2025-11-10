use {
    crate::{admin_rpc_service, cli::DefaultArgs, commands::{Error, Result}},
    clap::{App, Arg, ArgMatches, SubCommand},
    solana_clap_utils::input_parsers::values_of,
    solana_core::proxy::block_engine_stage::parse_block_engine_entry,
    std::path::Path,
};

pub fn command(_default_args: &DefaultArgs) -> App<'_, '_> {
    SubCommand::with_name("set-secondary-block-engine-urls")
        .about("Set secondary block engine entries")
        .arg(
            Arg::with_name("entries")
                .long("entries")
                .help("Secondary block engine entries as url,uuid. Set to empty string to remove all.")
                .takes_value(true)
                .multiple(true)
                .required(true),
        )
}

pub fn execute(subcommand_matches: &ArgMatches, ledger_path: &Path) -> Result<()> {
    let mut secondary_block_engine_entries = Vec::new();
    for entry in values_of::<String>(subcommand_matches, "entries").unwrap_or_default() {
        secondary_block_engine_entries.push(
            parse_block_engine_entry(&entry).map_err(|err| {
                Error::Dynamic(format!("invalid secondary block engine entry: {err}").into())
            })?,
        );
    }
    let admin_client = admin_rpc_service::connect(ledger_path);
    admin_rpc_service::runtime().block_on(async move {
        admin_client
            .await?
            .set_secondary_block_engine_urls(secondary_block_engine_entries)
            .await
    })?;
    Ok(())
}
