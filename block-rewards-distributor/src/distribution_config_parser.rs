use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// If any validator has stake accounts from the above stake pool IDs,
/// the rewards from those stake accounts go to the pool reserve (instead of stake account)
/// to avoid epoch delay.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DistributionConfig {
    /// Stake pool vote accounts whose stake rewards should be routed to the pool reserve.
    pub stake_pool_ids: Vec<String>,

    /// Validator-specific configuration keyed by validator vote account.
    pub validators_config: HashMap<String, ValidatorConfig>,
}

/// Validator-specific configuration keyed by vote account.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ValidatorConfig {
    /// Staker-level configuration (exclusions, custom commissions).
    pub stakers: Option<StakersConfig>,

    /// Validator commission BPS.
    /// Must be >= commission set in the RCA and <= 10000.
    pub validator_commission_bps: Option<u32>,

    /// Any remaining rewards after stakers + custom splits.
    /// If not provided, all remaining rewards go to validator identity.
    pub validator_reward_split: Option<ValidatorRewardSplit>,
}

/// Staker-level controls
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StakersConfig {
    /// These stake accounts are excluded and will NOT get rewards.
    ///
    /// All other stakers must receive at least:
    /// `10000 - validator_commission_bps`.
    ///
    /// Example:
    /// If validator commission = 7000 BPS (70%),
    /// each staker must receive at least 30% of rewards.
    pub excluded_stake_pubkeys: Option<Vec<String>>,

    /// Custom per-staker commission overrides.
    pub custom_commissions: Option<HashMap<String, CommissionConfig>>,
}

/// Custom commission for a specific staker
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CommissionConfig {
    /// The commission BPS for this staker.
    ///
    /// Examples: if validator_commission_bps = 7000
    /// - 7000 → Validator gets 70%, staker gets 30% (no need to specify)
    /// - 6000 → Validator gets 60%, staker gets 40%
    /// - 8000 → Requested 80% (invalid, lowered to 70%)
    /// - 0 → Validator gets 0%, staker gets 100%
    pub commission_bps: u32,

    /// Public key of referral claimant if wants to share staker rewards.
    #[serde(default)]
    pub referral_claimant_pubkey: Option<String>,

    /// Referral commission taken from staker’s portion.
    /// Example: 1000 BPS means 10% from staker share goes to referral
    #[serde(default)]
    pub referral_claimant_commission_bps: Option<u32>,
}

/// Validator-level remainder-split configuration
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ValidatorRewardSplit {
    /// The claimant receiving a portion of remaining rewards.
    pub claimant_pubkey: String,

    /// Percentage of remaining rewards going to claimant.
    ///
    /// Example: 5000 BPS → 50% claimant / 50% validator
    pub claimant_commission_bps: u32,
}

/// Maximum allowed commission in BPS
pub const MAX_COMMISSION_BPS: u32 = 10_000;

impl ValidatorConfig {
    pub fn validate_and_normalize(&mut self, validator: String, validator_commission_bps: u32) {
        // === Handle validator_commission_bps ===
        let mut effective_commission = validator_commission_bps;

        if let Some(v_comm) = &mut self.validator_commission_bps {
            let before = *v_comm;

            // Clamp to MAX_COMMISSION_BPS first
            if *v_comm > MAX_COMMISSION_BPS {
                *v_comm = MAX_COMMISSION_BPS;
                warn!(
                    "Validator {} config commission exceeded max BPS: {} → {}",
                    validator, before, *v_comm
                );
            }

            // Then clamp to on-chain validator commission limit
            if *v_comm > validator_commission_bps {
                *v_comm = validator_commission_bps;
                warn!(
                    "Validator {} commission capped to on-chain value: {} → {}",
                    validator, before, *v_comm
                );
            }

            effective_commission = *v_comm;
        }

        // === Handle stakers + custom commissions ===
        if let Some(stakers) = &mut self.stakers {
            if let Some(custom_commissions) = &mut stakers.custom_commissions {
                let mut to_remove = vec![];

                for (staker, cfg) in custom_commissions.iter_mut() {
                    let before = cfg.commission_bps;
                    info!("staker {:?} cfg {:?}", staker, cfg);

                    // Clamp staker commission: MAX_COMMISSION_BPS first, then effective_commission
                    if cfg.commission_bps > MAX_COMMISSION_BPS {
                        cfg.commission_bps = MAX_COMMISSION_BPS;
                        warn!(
                            "Validator {} staker {} commission capped: {} → {}",
                            validator, staker, before, cfg.commission_bps
                        );
                    } else if cfg.commission_bps > effective_commission {
                        cfg.commission_bps = effective_commission;
                        warn!(
                            "Validator {} staker {} commission lowered to validator limit: {} → {}",
                            validator, staker, before, cfg.commission_bps
                        );
                    }

                    // Clean referral if 0 or invalid
                    if let Some(bps) = cfg.referral_claimant_commission_bps {
                        if bps == 0 || cfg.referral_claimant_pubkey.is_none() {
                            cfg.referral_claimant_commission_bps = None;
                            cfg.referral_claimant_pubkey = None;
                            warn!(
                                "Validator {} staker {} referral removed (0 or missing pubkey)",
                                validator, staker
                            );
                        }
                    }

                    // Remove redundant config if default commission & no referral
                    if cfg.commission_bps == effective_commission
                        && cfg.referral_claimant_pubkey.is_none()
                        && cfg.referral_claimant_commission_bps.is_none()
                    {
                        to_remove.push(staker.clone());
                        warn!(
                            "Validator {} staker {} config removed (default + no referral)",
                            validator, staker
                        );
                    }
                }

                for key in to_remove {
                    custom_commissions.remove(&key);
                }

                if custom_commissions.is_empty() {
                    stakers.custom_commissions = None;
                }
            }

            // === Handle validator-level reward split ===
            if let Some(split) = &mut self.validator_reward_split {
                let before = split.claimant_commission_bps;

                if split.claimant_commission_bps > MAX_COMMISSION_BPS {
                    split.claimant_commission_bps = MAX_COMMISSION_BPS;
                    warn!(
                        "Validator {} claimant commission capped: {} → {}",
                        validator, before, split.claimant_commission_bps
                    );
                }

                if split.claimant_commission_bps == 0 {
                    warn!(
                        "Validator {} reward split removed (0 commission)",
                        validator
                    );
                    self.validator_reward_split = None;
                }
            }
        }
    }
}
