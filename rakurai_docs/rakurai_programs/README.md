# Rakurai Programs

A collection of Solana smart contracts and tools required for **Rakurai’s validator operations**.

Between them, these programs hold four things on-chain: **who may run the Rakurai scheduler and at what commission** (Activation), **which services can reach a validator** (Client Config), **how tips are collected and split** (Tip Manager), and **who is owed what across all four revenue streams** (Reward Distribution).

## 1. Documentation

### 1.1. Programs

| Section | Description |
| ------- | ----------- |
| [Rakurai Activation](./programs/rakurai_activation/README.md) | Multisig-controlled smart contract that authorizes and manages validators running Rakurai nodes. |
| [Reward Distribution](./programs/reward_distribution/README.md) | Four money flows: [RCA](./programs/reward_distribution/README.md#3-rca--block-rewards-for-stakers) (block rewards → stakers), [TCA](./programs/reward_distribution/README.md#4-tca--tips-for-landing-transactions) (tips), [PSA](./programs/reward_distribution/README.md#5-psa--prepaid-fee-to-use-post-pack) (P2C subscription), [MCA](./programs/reward_distribution/README.md#6-mca--sharing-post-pack-backrun-profit) (backrun share). |
| [Rakurai Tip Manager](./programs/rakurai_tip_manager/README.md) | Eight tip accounts; tips are split and the remainder goes to the validator’s [TCA](./programs/reward_distribution/README.md#4-tca--tips-for-landing-transactions). |
| [Rakurai Client Config](./programs/rakurai_client_config/README.md) | Scheduler config: block-engine (recv bundles), P2C (send txns for backrun), virtual-priority (percent of tip). |

### 1.2. CLIs

| Section | Description |
| ------- | ----------- |
| [Rakurai CLIs](./cli/README.md) | Overview, install, and index. |
| [Rakurai Activation CLI](./cli/activation.md) | RAA init, scheduler enable/disable, commission (`rakurai-activation`). |
| [P2C Subscription CLI](./cli/p2c_subscription.md) | PSA prepaid subscription (`rakurai-p2c`). |
| [Partner Tip and MevShare Revenue Settlement CLI](./cli/partner_reward_settlement.md) | Settle partner TCA / MCA (`rakurai-revshare`). |
| [Client Config CLI](./cli/client_config.md) | Block-engine / P2C / VP — global, per-vote, proposals. Always submit **current + new**  (`rakurai-client-config`). |
