use {
    borsh::BorshDeserialize,
    solana_account::{AccountSharedData, ReadableAccount},
    solana_clock::Epoch,
    solana_pubkey::Pubkey,
    thiserror::Error,
};

#[derive(Debug, PartialEq, Clone, Eq, Error)]
pub enum TipDistributionError {
    #[error("Invalid account owner")]
    InvalidAccountOwner,
    #[error("Invalid discriminator")]
    InvalidDiscriminator,
    #[error("Deserialization error")]
    DeserializationError,
    #[error("Serialization error")]
    SerializationError,
}

const HEADER_SIZE: usize = 8;

pub type TipDistributionResult<T> = std::result::Result<T, TipDistributionError>;

/// Reads `payload_len` Borsh bytes after the 8-byte discriminator. `payload_len` is the
/// packed field size, not `size_of::<T>()` (that includes Rust alignment padding).
fn read_borsh_account<T: BorshDeserialize>(
    account_shared_data: &AccountSharedData,
    program_id: &Pubkey,
    discriminator: &[u8],
    payload_len: usize,
    label: &str,
) -> TipDistributionResult<T> {
    if account_shared_data.owner() != program_id {
        return Err(TipDistributionError::InvalidAccountOwner);
    }

    let data = account_shared_data.data();
    let end = HEADER_SIZE + payload_len;
    if data.len() < end || &data[..HEADER_SIZE] != discriminator {
        return Err(TipDistributionError::InvalidDiscriminator);
    }

    let mut payload = &data[HEADER_SIZE..end];
    T::deserialize(&mut payload).map_err(|e| {
        error!("Error deserializing {label}: {e}");
        TipDistributionError::DeserializationError
    })
}

#[allow(unused)]
#[derive(BorshDeserialize)]

pub struct TipDistributionAccount {
    /// The validator's vote account, also the recipient of remaining lamports after
    /// upon closing this account.
    pub validator_vote_account: Pubkey,

    /// The only account authorized to upload a merkle-root for this account.
    pub merkle_root_upload_authority: Pubkey,

    /// The merkle root used to verify user claims from this account.
    pub merkle_root: Option<MerkleRoot>,

    /// Epoch for which this account was created.  
    pub epoch_created_at: u64,

    /// The commission basis points this validator charges.
    pub validator_commission_bps: u16,

    /// The epoch (upto and including) that tip funds can be claimed.
    pub expires_at: u64,

    /// The bump used to generate this account
    pub bump: u8,
}

#[allow(unused)]
#[derive(BorshDeserialize)]
pub struct MerkleRoot {
    /// The 256-bit merkle root.
    pub root: [u8; 32],

    /// Maximum number of funds that can ever be claimed from this [MerkleRoot].
    pub max_total_claim: u64,

    /// Maximum number of nodes that can ever be claimed from this [MerkleRoot].
    pub max_num_nodes: u64,

    /// Total funds that have been claimed.
    pub total_funds_claimed: u64,

    /// Number of nodes that have been claimed.
    pub num_nodes_claimed: u64,
}

impl TipDistributionAccount {
    const DISCRIMINATOR: &'static [u8] = &[85, 64, 113, 198, 234, 94, 120, 123];
    /// vote + upload authority + Option<MerkleRoot> + epoch + commission + expires + bump.
    /// 32 + 32 + (1 + 64) + 8 + 2 + 8 + 1 = 148. Sized for `merkle_root = Some`.
    const PAYLOAD_LEN: usize = 148;

    pub(crate) fn find_program_address(
        program_id: &Pubkey,
        vote_pubkey: &Pubkey,
        epoch: Epoch,
    ) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                b"TIP_DISTRIBUTION_ACCOUNT",
                vote_pubkey.to_bytes().as_ref(),
                epoch.to_le_bytes().as_ref(),
            ],
            program_id,
        )
    }

    pub(crate) fn from_account_shared_data(
        account_shared_data: &AccountSharedData,
        program_id: &Pubkey,
    ) -> TipDistributionResult<Self> {
        read_borsh_account(
            account_shared_data,
            program_id,
            Self::DISCRIMINATOR,
            Self::PAYLOAD_LEN,
            "tip distribution account",
        )
    }
}

pub struct InitializeTipDistributionConfigInstruction;

impl InitializeTipDistributionConfigInstruction {
    const DISCRIMINATOR: &'static [u8] = &[175, 175, 109, 31, 13, 152, 155, 237];

    pub(crate) fn to_instruction_data(
        authority: Pubkey,
        expired_funds_account: Pubkey,
        num_epochs_valid: u64,
        max_validator_commission_bps: u16,
        bump: u8,
    ) -> TipDistributionResult<Vec<u8>> {
        let mut data = Vec::with_capacity(Self::DISCRIMINATOR.len() + 75);
        data.extend_from_slice(Self::DISCRIMINATOR);
        data.extend(borsh::to_vec(&authority).map_err(|e| {
            error!("Error serializing authority: {e}");
            TipDistributionError::SerializationError
        })?);
        data.extend(borsh::to_vec(&expired_funds_account).map_err(|e| {
            error!("Error serializing expired funds account: {e}");
            TipDistributionError::SerializationError
        })?);
        data.extend(borsh::to_vec(&num_epochs_valid).map_err(|e| {
            error!("Error serializing num epochs valid: {e}");
            TipDistributionError::SerializationError
        })?);
        data.extend(borsh::to_vec(&max_validator_commission_bps).map_err(|e| {
            error!("Error serializing max validator commission bps: {e}");
            TipDistributionError::SerializationError
        })?);
        data.extend(borsh::to_vec(&bump).map_err(|e| {
            error!("Error serializing bump: {e}");
            TipDistributionError::SerializationError
        })?);
        Ok(data)
    }
}

pub struct InitializeTipDistributionAccountInstruction;

impl InitializeTipDistributionAccountInstruction {
    const DISCRIMINATOR: &'static [u8] = &[120, 191, 25, 182, 111, 49, 179, 55];

    pub(crate) fn to_instruction_data(
        merkle_root_upload_authority: Pubkey,
        validator_commission_bps: u16,
        bump: u8,
    ) -> TipDistributionResult<Vec<u8>> {
        let mut data = Vec::with_capacity(Self::DISCRIMINATOR.len() + 35);
        data.extend_from_slice(Self::DISCRIMINATOR);
        data.extend(borsh::to_vec(&merkle_root_upload_authority).map_err(|e| {
            error!("Error serializing merkle root upload authority: {e}");
            TipDistributionError::SerializationError
        })?);
        data.extend(borsh::to_vec(&validator_commission_bps).map_err(|e| {
            error!("Error serializing validator commission bps: {e}");
            TipDistributionError::SerializationError
        })?);
        data.extend(borsh::to_vec(&bump).map_err(|e| {
            error!("Error serializing bump: {e}");
            TipDistributionError::SerializationError
        })?);

        Ok(data)
    }
}

#[allow(unused)]
#[derive(BorshDeserialize)]
pub struct JitoTipDistributionConfig {
    /// Account with authority over this PDA.
    authority: Pubkey,

    /// We want to expire funds after some time so that validators can be refunded the rent.
    /// Expired funds will get transferred to this account.
    expired_funds_account: Pubkey,

    /// Specifies the number of epochs a merkle root is valid for before expiring.
    num_epochs_valid: u64,

    /// The maximum commission a validator can set on their distribution account.
    max_validator_commission_bps: u16,

    /// The bump used to generate this account
    bump: u8,
}

#[allow(unused)]
impl JitoTipDistributionConfig {
    const DISCRIMINATOR: &'static [u8] = &[155, 12, 170, 224, 30, 250, 204, 130];

    pub(crate) fn from_account_shared_data(
        account_shared_data: &AccountSharedData,
        program_id: &Pubkey,
    ) -> TipDistributionResult<Self> {
        if account_shared_data.owner() != program_id {
            return Err(TipDistributionError::InvalidAccountOwner);
        }

        if &account_shared_data.data()[0..8] != Self::DISCRIMINATOR {
            return Err(TipDistributionError::InvalidDiscriminator);
        }

        JitoTipDistributionConfig::try_from_slice(&account_shared_data.data()[8..83]).map_err(|e| {
            error!("Error deserializing tip distribution config account: {e}");
            TipDistributionError::DeserializationError
        })
    }

    pub(crate) fn find_program_address(program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"CONFIG_ACCOUNT"], program_id)
    }

    pub(crate) fn authority(&self) -> Pubkey {
        self.authority
    }

    pub(crate) fn expired_funds_account(&self) -> Pubkey {
        self.expired_funds_account
    }

    pub(crate) fn num_epochs_valid(&self) -> u64 {
        self.num_epochs_valid
    }

    pub(crate) fn max_validator_commission_bps(&self) -> u16 {
        self.max_validator_commission_bps
    }

    pub(crate) fn bump(&self) -> u8 {
        self.bump
    }
}

#[allow(unused)]
#[derive(BorshDeserialize)]
pub struct ClaimStatus {
    /// Whether the claim was already made.
    pub is_claimed: bool,
    /// Who made the claim.
    pub claimant: Pubkey,
    /// Payer of the claim status account.
    pub claim_status_payer: Pubkey,
    /// Slot when the claim was made.
    pub slot_claimed_at: u64,
    /// Amount claimed.
    pub amount: u64,
    /// Expiry of this claim.
    pub expires_at: u64,
    /// PDA bump.
    pub bump: u8,
}

impl ClaimStatus {
    /// PDA seed for claim status accounts.
    pub const SEED: &'static [u8] = b"CLAIM_STATUS";
    const DISCRIMINATOR: &'static [u8] = &[22, 183, 249, 157, 247, 95, 150, 96];
    /// is_claimed + claimant + payer + slot + amount + expires + bump.
    /// 1 + 32 + 32 + 8 + 8 + 8 + 1 = 90.
    const PAYLOAD_LEN: usize = 90;

    pub(crate) fn from_account_shared_data(
        account_shared_data: &AccountSharedData,
        program_id: &Pubkey,
    ) -> TipDistributionResult<Self> {
        read_borsh_account(
            account_shared_data,
            program_id,
            Self::DISCRIMINATOR,
            Self::PAYLOAD_LEN,
            "claim status account",
        )
    }
}
