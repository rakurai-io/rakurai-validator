use {
    solana_account::ReadableAccount, solana_pubkey::Pubkey, solana_runtime::bank::Bank,
    std::collections::HashMap,
};

#[derive(Clone, Debug)]
pub struct CachedUuidTipGroup {
    pub uuid: String,
    pub uuid_name: [u8; 32],
    pub entries: Vec<(Pubkey, f64)>,
}

#[derive(Clone, Debug)]
pub struct TipUuidDelta {
    pub uuid: String,
    pub uuid_name: [u8; 32],
    pub amount: u64,
}

#[cfg(feature = "build_validator")]
mod ffi {
    use {super::CachedUuidTipGroup, solana_pubkey::Pubkey, solana_runtime::bank::Bank};

    unsafe extern "C" {
        #[allow(improper_ctypes)]
        #[allow(improper_ctypes_definitions)]
        pub fn load_cached_uuid_tip_groups(
            bank: &Bank,
            vote_account: &Pubkey,
        ) -> Option<Vec<CachedUuidTipGroup>>;
    }
}

pub fn load_cached_uuid_tip_groups(
    bank: &Bank,
    vote_account: &Pubkey,
) -> Option<Vec<CachedUuidTipGroup>> {
    #[cfg(feature = "build_validator")]
    {
        // SAFETY: exported by rakurai_scheduler entrypoint from the same revision.
        return unsafe { ffi::load_cached_uuid_tip_groups(bank, vote_account) };
    }
    #[cfg(not(feature = "build_validator"))]
    {
        let _ = (bank, vote_account);
        None
    }
}

fn account_lamports(bank: &Bank, pubkey: &Pubkey) -> u64 {
    bank.get_account(pubkey)
        .map(|account| account.lamports())
        .unwrap_or(0)
}

fn weighted_lamport_delta(start_lamports: u64, end_lamports: u64, value: f64) -> u64 {
    let delta = end_lamports.saturating_sub(start_lamports);
    if value == 1.0 {
        delta
    } else {
        (delta as f64 * value) as u64
    }
}

/// Snapshots the current lamport balance of every tip account referenced by `groups`.
///
/// Balances must be captured while the source bank is still retained in `bank_forks`
/// (i.e. before the root advances past it and prunes it), because pruned banks can no
/// longer be read back.
pub fn snapshot_group_balances(bank: &Bank, groups: &[CachedUuidTipGroup]) -> HashMap<Pubkey, u64> {
    let mut balances = HashMap::new();
    for group in groups {
        for (pubkey, _) in &group.entries {
            balances
                .entry(*pubkey)
                .or_insert_with(|| account_lamports(bank, pubkey));
        }
    }
    balances
}

/// Computes the weighted per-UUID tip delta from two previously captured balance snapshots.
pub fn weighted_tip_deltas_from_balances(
    start_balances: &HashMap<Pubkey, u64>,
    end_balances: &HashMap<Pubkey, u64>,
    groups: &[CachedUuidTipGroup],
) -> Vec<TipUuidDelta> {
    groups
        .iter()
        .map(|group| {
            let amount = group.entries.iter().fold(0u64, |acc, (pubkey, value)| {
                let start = start_balances.get(pubkey).copied().unwrap_or(0);
                let end = end_balances.get(pubkey).copied().unwrap_or(0);
                acc.saturating_add(weighted_lamport_delta(start, end, *value))
            });
            TipUuidDelta {
                uuid: group.uuid.clone(),
                uuid_name: group.uuid_name,
                amount,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weighted_lamport_delta() {
        assert_eq!(weighted_lamport_delta(100, 200, 1.0), 100);
        assert_eq!(weighted_lamport_delta(100, 200, 0.5), 50);
        assert_eq!(weighted_lamport_delta(200, 100, 1.0), 0);
    }
}
