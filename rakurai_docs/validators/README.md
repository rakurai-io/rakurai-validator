# Rakurai Validator Operators

**Running Rakurai turns one revenue stream into four.** Alongside block rewards, a Rakurai validator earns tips from traders competing to land, prepaid subscriptions from services consuming its post-pack stream, and a share of the backrun profit those services make — each metered in its own on-chain account.

Rakurai is a fork of jito-solana running the Rakurai scheduler library, so it replaces your client rather than adding a sidecar. Rakurai currently charges **no fee for running it**, and you choose how much of the block reward to share with stakers through a Merkle-root-based distribution you can hand to Rakurai at 0% distribution fee.

Operationally: build the client, create an on-chain activation account, and keep the scheduler binary in step with each release. Those, plus Geyser compatibility, are what the guides below cover.

**Audience:** Solana validator operators who want to run the Rakurai client and maximize block rewards.

---

## 1. Where to start

| If you are… | Go to |
|-------------|-------|
| Setting up Rakurai for the first time | [Setup and build](./setup_and_build.md) |
| Already running Rakurai and moving to a new release | [Node upgrade](./node_upgrade.md) |
| Deciding whether to share block rewards with stakers | [Block reward distribution](../../block-rewards-distributor/block_reward_distribution.md) |
| Running a Geyser plugin alongside Rakurai | [Geyser](./geyser.md) |
| Verifying a downloaded scheduler binary | [Binary attestation](./binary_attestation.md) |

> [!WARNING]
> **Three things that will break a Rakurai node**
>
> - Running a **Geyser plugin that was not rebuilt** against the Rakurai crates — [Geyser](./geyser.md).
> - Running a **scheduler binary from a different release** than the client — [Node upgrade](./node_upgrade.md).
> - Losing **`LD_LIBRARY_PATH`** or **XDP capabilities** after a rebuild — [Setup and build](./setup_and_build.md#3-build-the-client).

---

## 2. Guides

| Guide | Description |
|-------|-------------|
| [Setup and build](./setup_and_build.md) | Prerequisites, activation account, scheduler binary, build, and CLI args |
| [Node Upgrade](./node_upgrade.md) | Fast upgrade path for existing Rakurai operators |
| [Block Reward Distribution](../../block-rewards-distributor/block_reward_distribution.md) | Share block rewards with stakers via the RCA, and customize allocations |
| [Geyser](./geyser.md) | Build and run Geyser plugins compatible with Rakurai |
| [Binary attestation](./verify_rakurai_binary.md) | Verify the scheduler binary's attestations (provenance and SBOM) and checksum |
| [Spark geyser](../../spark-geyser/README.md) | Prebuilt sample Geyser plugin (ZeroMQ forwarding) |

---

## 3. What a Rakurai validator earns

Four separate revenue streams reach a Rakurai validator, each with its own on-chain account. The full model is in [Reward Distribution](../rakurai_programs/programs/reward_distribution/README.md).

| Stream | Account | Where it comes from |
|--------|---------|---------------------|
| Block rewards | [RCA](../rakurai_programs/programs/reward_distribution/README.md#3-rca--block-rewards-for-stakers) | Leader slots; the staker share is distributed after the epoch |
| Tips | [TCA](../rakurai_programs/programs/reward_distribution/README.md#4-tca--tips-for-landing-transactions) | Traders and landing services tipping to get prioritized |
| Post-pack access | [PSA](../rakurai_programs/programs/reward_distribution/README.md#5-psa--prepaid-fee-to-use-post-pack) | Prepaid subscriptions from post-pack consumers, priced from your stake |
| Backrun share | [MCA](../rakurai_programs/programs/reward_distribution/README.md#6-mca--sharing-post-pack-backrun-profit) | A share of profit searchers make from your post-pack stream |

Commission on block rewards is set per validator in the [Rakurai Activation Account](../rakurai_programs/programs/rakurai_activation/README.md) with [`rakurai-activation`](../rakurai_programs/cli/activation.md).

---

## 4. Related integrator docs

You do not need these to run a node, but they explain the traffic your validator receives:

- [TIN - MEV Services](../transaction_inclusion/README.md) — hub for bundles, tips, and post-pack
- [Setup guide](../transaction_inclusion/setup_guide.md) — block engine discovery, gRPC roles, onboarding
- [Tips](../transaction_inclusion/tips.md) — how tips and virtual priority work on Rakurai nodes
- [Post-pack confirmations](../transaction_inclusion/post_pack/README.md) — Admin RPC for endpoints; **PSA** then **MCA**
- [P2C Subscription CLI](../rakurai_programs/cli/p2c_subscription.md) — top up the **PSA** prepaid subscription (`rakurai-p2c`)
- [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md) — partner TCA / MCA inspect and settle (`rakurai-revshare`)
