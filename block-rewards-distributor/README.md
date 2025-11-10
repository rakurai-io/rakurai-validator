# Block Rewards Distributor

A collection of binaries used to distribute Block rewards to stakers and validators on using Merkle tree-based distribution.

## Overview

This system automates the distribution of block rewards to stakers and validators. It uses Merkle trees to enable efficient, verifiable reward claims on-chain. The workflow consists of four sequential binaries that process ledger snapshots, generate distribution merkle trees, upload merkle root on-chain, and execute claims.

This system works with the [Rakurai Reward Distribution Program](https://docs.rakurai.io/l_rakurai_reward_distribution) to distribute validator block rewards.

## Architecture

The distribution process follows these steps:

1. **Stake Meta Generation** - Extract stake account and delegation data from ledger snapshot
2. **Merkle Root Generation** - Build Merkle trees with reward calculations based on configuration
3. **Merkle Root Upload** - Upload Merkle roots to on-chain Reward Collection Accounts (RCAs)
4. **Claim Rewards** - Execute reward claims for all eligible recipients

## Binaries

### 1. `stake-meta-generator`

Generates metadata about stake accounts and delegations from a Solana ledger snapshot.

**Purpose:**
- Loads a Solana snapshot at Epoch last slot
- Scans all stake accounts and their delegations to validators
- Identifies Reward Collection Accounts (RCAs) for validators with block rewards
- Outputs structured JSON metadata for downstream processing

**Usage:**
```bash
stake-meta-generator \
  --ledger-path /path/to/ledger \
  --reward-distribution-program-id <PROGRAM_ID> \
  --out-path stake_meta.json \
  --snapshot-slot <SLOT>
```

**Arguments:**
- `--ledger-path`: Path to the Solana ledger directory containing the snapshot
- `--reward-distribution-program-id`: The reward distribution program ID (Pubkey). See [deployed program IDs](https://docs.rakurai.io/nodeoperator#deployed-program-id) for mainnet and testnet addresses.
- `--out-path`: Output path for the generated `StakeMetaCollection` JSON file
- `--snapshot-slot`: The slot number of the snapshot to process

**Output:** `StakeMetaCollection` JSON file containing:
- List of validators with their delegations
- Reward Collection Account metadata
- Total delegated amounts per validator
- Validator commission rates

---

### 2. `merkle-root-generator`

Generates Merkle trees for reward distribution based on stake metadata and distribution configuration.

**Purpose:**
- Reads stake metadata from step 1
- Applies distribution configuration (commissions, exclusions, splits)
- Calculates reward amounts for each staker, validator, and referral
- Builds Merkle trees with cryptographic proofs
- Validates calculations against on-chain RCA balances

**Usage:**
```bash
merkle-root-generator \
  --stake-meta-coll-path stake_meta.json \
  --distribution-config-path distribution_config.json \
  --rpc-url <RPC_URL> \
  --out-path merkle_trees.json
```

**Arguments:**
- `--stake-meta-coll-path`: Path to the `StakeMetaCollection` JSON from step 1
- `--distribution-config-path`: Path to the distribution configuration JSON (see Configuration section)
- `--rpc-url`: Solana RPC endpoint URL for on-chain validation
- `--out-path`: Output path for the generated `GeneratedMerkleTreeCollection` JSON

**Output:** `GeneratedMerkleTreeCollection` JSON file containing:
- Merkle root for each validator's reward distribution
- Tree nodes with reward amounts and Merkle proofs
- Claim status account addresses (PDAs)
- Total claimable amounts per validator

---

### 3. `merkle-root-uploader`

Uploads Merkle roots to on-chain Reward Collection Accounts.

**Purpose:**
- Reads generated Merkle tree data
- Uploads Merkle roots to each validator's Reward Collection Account
- Enables on-chain reward claims by making roots available

**Important:** Only the merkle root upload authority specified in the Reward Collection Account (RCA) can upload the merkle root. Ensure the keypair provided matches the `merkle_root_upload_authority` field in the RCA. 

If you set the `--rewards-merkle-root-authority` to Rakurai's address (`H21wFgN53ghjDq5N9QhraAiPn1tRVYkobySj55unXLEj`), Rakurai will automatically distribute rewards. See [Rakurai validator setup](https://docs.rakurai.io/nodeoperator#step-5-add-additional-cli-args) for more details.

**Usage:**
```bash
merkle-root-uploader \
  --merkle-root-path merkle_trees.json \
  --keypair-path /path/to/keypair.json \
  --rpc-url <RPC_URL> \
  --reward-distribution-program-id <PROGRAM_ID>
```

**Arguments:**
- `--merkle-root-path`: Path to the `GeneratedMerkleTreeCollection` JSON from step 2
- `--keypair-path`: Path to keypair file for signing transactions (must match RCA's merkle root upload authority)
- `--rpc-url`: Solana RPC endpoint URL
- `--reward-distribution-program-id`: The reward distribution program ID. See [deployed program IDs](https://docs.rakurai.io/nodeoperator#deployed-program-id) for mainnet and testnet addresses.

---

### 4. `claim-rewards`

Executes reward claims for all eligible recipients.

**Purpose:**
- Reads Merkle tree data with proofs
- Creates claim transactions for stakers, validators, referrals, and expenses
- Sends transactions with retry logic and error handling
- Optionally reclaims rent from closed ClaimStatus accounts

**Usage:**
```bash
claim-rewards \
  --merkle-trees-path merkle_trees.json \
  --rpc-url <RPC_URL> \
  --reward-distribution-program-id <PROGRAM_ID> \
  --keypair-path /path/to/keypair.json
```

**Arguments:**
- `--merkle-trees-path`: Path to the `GeneratedMerkleTreeCollection` JSON from step 2
- `--rpc-url`: Solana RPC endpoint URL
- `--reward-distribution-program-id`: The reward distribution program ID. See [deployed program IDs](https://docs.rakurai.io/nodeoperator#deployed-program-id) for mainnet and testnet addresses.
- `--keypair-path`: Path to keypair file for signing transactions

---

## Configuration

The distribution behavior is controlled by a JSON configuration file. See `.example.distribution_config.json` for a complete example.

**Important Timing Note:** If the merkle root upload authority in your Reward Collection Account (RCA) is set to Rakurai (`H21wFgN53ghjDq5N9QhraAiPn1tRVYkobySj55unXLEj`) and you want to use custom distribution configuration, you **must** share your distribution config before the epoch ends. This is because rewards from epoch X are distributed in epoch X+1, so your configuration must be provided before epoch X ends to be applied to those rewards. See [Rakurai validator setup](https://docs.rakurai.io/nodeoperator#step-5-add-additional-cli-args) for details on setting the merkle root authority.

### Configuration Structure

```json
{
  "stake_pool_ids": ["..."],
  "validators_config": {
    "<validator_vote_account>": {
      "stakers": { ... },
      "validator_reward_split": { ... }
    }
  }
}
```

### Top-Level Settings

#### `stake_pool_ids`
Array of stake pool program IDs. If a validator has stake accounts from these pools, rewards are routed to the pool reserve account instead of the stake account to avoid epoch delay.

### Validator-Specific Configuration

Each validator can be configured by their vote account address:

#### `stakers` (optional)
Staker-level configuration:

- **`excluded_stake_pubkeys`**: Array of stake account or staker pubkeys that will NOT receive rewards. All other stakers must receive at least `(10000 - validator_commission_bps)` BPS of rewards.

- **`custom_commissions`**: Per-staker commission overrides keyed by stake account pubkey:
  - `commission_bps`: Validator commission in basis points (0-10000). Example: 6000 = 60% to validator, 40% to staker
  - `referral_claimant_pubkey`: Optional referral recipient address
  - `referral_claimant_commission_bps`: Referral percentage from staker's share (0-10000 BPS)

#### `validator_reward_split` (optional)
Splits remaining validator rewards between validator identity and a claimant:

- `claimant_pubkey`: Address receiving a portion of validator rewards
- `claimant_commission_bps`: Percentage (0-10000 BPS) going to claimant. Remaining goes to validator identity.

If not specified, all remaining rewards go to the validator identity account.

### Configuration Examples

**Exclude specific stakers:**
```json
{
  "validators_config": {
    "<vote_account>": {
      "stakers": {
        "excluded_stake_pubkeys": ["<stake_pubkey_1>", "<stake_pubkey_2>"]
      }
    }
  }
}
```

**Custom commission per staker:**
```json
{
  "validators_config": {
    "<vote_account>": {
      "stakers": {
        "custom_commissions": {
          "<stake_account>": {
            "commission_bps": 6000,
            "comment": "Validator gets 60%, staker gets 40%"
          }
        }
      }
    }
  }
}
```

**Referral rewards:**
```json
{
  "validators_config": {
    "<vote_account>": {
      "stakers": {
        "custom_commissions": {
          "<stake_account>": {
            "commission_bps": 6000,
            "referral_claimant_pubkey": "<referral_address>",
            "referral_claimant_commission_bps": 1000,
            "comment": "Validator gets 60%. From staker's 40%, 10% goes to referral"
          }
        }
      }
    }
  }
}
```

**Validator reward split:**
```json
{
  "validators_config": {
    "<vote_account>": {
      "validator_reward_split": {
        "claimant_pubkey": "<claimant_address>",
        "claimant_commission_bps": 5000,
        "comment": "50% to claimant, 50% to validator identity"
      }
    }
  }
}
```

### Configuration Validation

The system automatically validates and normalizes configuration:
- Commissions exceeding maximum (10000 BPS) are capped
- Commissions exceeding on-chain validator commission are lowered to match
- Invalid referral configurations (0 BPS or missing pubkey) are removed
- Redundant configurations matching defaults are removed

---

## Setup

### Building

From the workspace root:
```bash
cargo build --release
```

Binaries will be available in `target/release/`:
- `stake-meta-generator`
- `merkle-root-generator`
- `merkle-root-uploader`
- `claim-rewards`

---

## Notes

- All amounts are in lamports (1 SOL = 1,000,000,000 lamports)
- Commission rates are in basis points (BPS), where 10000 BPS = 100%
- Merkle roots must be uploaded before claims can be executed
- The keypair must have sufficient SOL for transaction fees
- The system automatically calculates transaction fees and includes them in the expense node

---

