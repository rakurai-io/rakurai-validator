# Rakurai Validator Operators

Documentation for running and operating a Rakurai-Solana validator node.

**Audience:** Solana validator operators who want to run the Rakurai client and maximize block rewards.

---

## 1. Guides

| Guide | Description |
|-------|-------------|
| [Setup and build](./setup_and_build.md) | Prerequisites, activation account, scheduler binary, and CLI args |
| [Node Upgrade](./node_upgrade.md) | Fast upgrade path for existing Rakurai operators |
| [Block Reward Distribution](../../block-rewards-distributor/block_reward_distribution.md) | Share block rewards with stakers via Merkle-root-based distribution |
| [Geyser](./geyser.md) | Build and run Geyser plugins compatible with Rakurai |
| [Binary attestation](./binary_attestation.md) | Verify scheduler binary with GitHub artifact attestations |
| [Spark geyser](../../spark-geyser/README.md) | Prebuilt sample Geyser plugin (ZeroMQ forwarding) |

---

## 2. Related integrator docs

Validators may also need:

- [Post-pack confirmations](../transaction_inclusion/post_pack_confirmations.md) — Admin RPC for endpoint configuration
- [Transaction inclusion](../transaction_inclusion/transaction_inclusion.md) — Block engine and post-pack overview
- [Tips FAQ](../transaction_inclusion/rakurai_tip_manager_faqs.md) — How tips work on Rakurai nodes
- [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md) — Partner TCA/MCA inspect and settle (`rakurai-partner-settle`)
