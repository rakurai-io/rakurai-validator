use {solana_pubkey::Pubkey, solana_runtime::bank::Bank};

#[derive(Clone, Debug)]
pub struct CachedUuidMevShareGroup {
    pub uuid: String,
    pub uuid_name: [u8; 32],
}

#[cfg(feature = "build_validator")]
mod ffi {
    use {super::CachedUuidMevShareGroup, solana_pubkey::Pubkey, solana_runtime::bank::Bank};

    unsafe extern "C" {
        #[allow(improper_ctypes)]
        #[allow(improper_ctypes_definitions)]
        pub fn load_cached_uuid_mev_share_groups(
            bank: &Bank,
            vote_account: &Pubkey,
        ) -> Option<Vec<CachedUuidMevShareGroup>>;
    }
}

/// Loads unique service UUIDs from the on-chain post-pack confirmation config.
/// These UUIDs identify MEV-share collection accounts (MCAs).
pub fn load_cached_uuid_mev_share_groups(
    bank: &Bank,
    vote_account: &Pubkey,
) -> Option<Vec<CachedUuidMevShareGroup>> {
    #[cfg(feature = "build_validator")]
    {
        // SAFETY: exported by rakurai_scheduler entrypoint from the same revision.
        return unsafe { ffi::load_cached_uuid_mev_share_groups(bank, vote_account) };
    }
    #[cfg(not(feature = "build_validator"))]
    {
        let _ = (bank, vote_account);
        None
    }
}
