# Rakurai Programs

A collection of Solana smart contracts and tools required for **Rakurai’s validator operations**.

## 1. Documentation

### Programs

| Section | Description |
| ------- | ----------- |
| [Rakurai Activation](./programs/rakurai_activation/README.md) | Multisig-controlled smart contract that authorizes and manages validators running Rakurai nodes. |
| [Reward Distribution](./programs/reward_distribution/README.md) | Distributes block rewards to stakers via per-epoch RCA and post-epoch Merkle claims; tracks on-chain tip and MevShare revenue in per-validator, per-service [Tips Collection Accounts and MevShare Collection Accounts](./programs/reward_distribution/README.md#5-tip-and-mevshare-collection-accounts) (TCA / MCA). |
| [Rakurai Tip Manager](./programs/rakurai_tip_manager/README.md) | Manages tips sent to Rakurai validators across eight tip PDAs; drains and splits tips to the validator's [Tips Collection Account](./programs/reward_distribution/README.md#51-why-a-tips-collection-account-tca) (TCA). |

### CLIs

| Section | Description |
| ------- | ----------- |
| [Rakurai CLIs](./cli/README.md) | Overview, install, and index for both CLI binaries. |
| [Rakurai Activation CLI](./cli/activation.md) | Operator CLI for Rakurai Activation Accounts — initialize an [RAA](./programs/rakurai_activation/README.md#4-rakurai-activation-account-creation), enable/disable the scheduler, and update commission. |
| [Partner Tip and MevShare Revenue Settlement CLI](./cli/partner_reward_settlement.md) | List/record MCA MevShare (post-pack) and settle custom tip (TCA) or post-pack (MCA) revenue, per vault or all at once (`rakurai-revshare`). |
