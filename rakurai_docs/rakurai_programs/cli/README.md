# Rakurai CLIs

Command-line tools for Rakurai validator operators and TIN partners.

**Four binaries, split by who pays whom.** Validator operators use `rakurai-activation` to join Rakurai and set their commission. TIN partners use `rakurai-p2c` to keep a post-pack subscription funded and `rakurai-revshare` to settle what they owe each epoch. Rakurai ops and validator operators use `rakurai-client-config` to control which services can reach a node.

**Audience:** Validator operators, transaction-landing services, and post-pack user.

---

## 1. Overview

The `rakurai_cli` crate ships four binaries:

| Binary | Audience | Purpose |
| ------ | -------- | ------- |
| `rakurai-activation` | Validator operators | Manage Rakurai Activation Accounts (RAA): init, scheduler control, commission, show |
| `rakurai-p2c` | P2C User/Consumer | PSA prepaid subscription: inspect, fund, fund-all |
| `rakurai-revshare` | Transaction-landing / post-pack partners | Partner TCA (custom tip) and MCA (MevShare) settlement |
| `rakurai-client-config` | Rakurai ops / validator operators | Block-engine (recv bundles), P2C (send for backrun), virtual-priority (% of tip) — **full payload (current + new)** |

---

## 2. Installation

Download the latest prebuilt CLIs from the [`rakurai_programs` releases](https://github.com/rakurai-io/rakurai_programs/releases/latest), or build from source as below.

Ensure you have **[Rust and Cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html#install-rust-and-cargo)** installed before building from source.

You can either **build from source** or use the **prebuilt binary from the `release/downloads` directory**.

### 2.1. Option 1: Use prebuilt CLI

```bash
# Export the prebuilt CLI binary to your PATH
echo "export PATH=\"$(pwd)/release/downloads:\$PATH\"" >> ~/.bashrc && source ~/.bashrc
```

### 2.2. Option 2: Build from source

```sh
# Build CLI binaries
cargo b --release -p rakurai_cli

# Export the CLI path
echo "export PATH=\"$(pwd)/target/release/:\$PATH\""
```

### 2.3. Verify installation

```sh
which rakurai-activation
which rakurai-revshare
which rakurai-p2c
which rakurai-client-config
```

---

## 3. Documentation

| Guide | Description |
| ----- | ----------- |
| [Rakurai Activation CLI](./activation.md) | Initialize and manage Rakurai Activation Accounts (RAA): scheduler enable/disable, commission updates, and account display. |
| [P2C Subscription CLI](./p2c_subscription.md) | Fund PSA prepaid escrow (`rakurai-p2c`). |
| [Partner Tip and MevShare Revenue Settlement CLI](./partner_reward_settlement.md) | Tip settle vs Mev-share record+settle (`rakurai-revshare`). |
| [Client Config CLI](./client_config.md) | Block-engine (recv bundles), P2C (send for backrun), virtual-priority (% of tip). Writes replace the whole config — submit **current + new**. |
