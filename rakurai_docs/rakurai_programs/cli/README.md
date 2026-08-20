# Rakurai CLIs

Command-line tools for Rakurai validator operators and TIN partners.

**Audience:** Validator operators, transaction-landing services, and post-pack user.

---

## 1. Overview

The `rakurai_cli` crate ships four binaries:

| Binary | Audience | Purpose |
| ------ | -------- | ------- |
| `rakurai-activation` | Validator operators | Manage Rakurai Activation Accounts (RAA): init, scheduler control, commission, show |
| `rakurai-p2c` | P2C User/Consumer | PSA prepaid subscription (stake-based fee): fund, record, claim, clear-deficit |
| `rakurai-revshare` | Transaction-landing / post-pack partners | Partner TCA (custom tip) and MCA (MevShare) settlement |
| `rakurai-client-config` | Rakurai ops / validator operators | Block-engine (recv bundles), P2C (send for backrun), virtual-priority (% of tip) — **full payload (current + new)** |

---

## 2. Installation

Ensure you have **[Rust and Cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html#install-rust-and-cargo)** installed before proceeding.

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
| [P2C Subscription CLI](./p2c_subscription.md) | PSA prepaid escrow: fund → record → claim (`rakurai-p2c`). |
| [Partner Tip and MevShare Revenue Settlement CLI](./partner_reward_settlement.md) | List TCA/MCA, record MCA MevShare, settle (`rakurai-revshare`). |
| [Client Config CLI](./client_config.md) | Block-engine (recv bundles), P2C (send for backrun), virtual-priority (% of tip). Writes replace the whole config — submit **current + new**. |
