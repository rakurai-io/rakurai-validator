# Rakurai Activation CLI

Command-line tool for managing the Rakurai Activation Account (RAA).

**Audience:** Solana validator operators running Rakurai nodes.

---

## 1. Overview

The **Rakurai Activation CLI** manages the Rakurai activation setup for validator operators. It interacts with a **multisig-based Solana smart contract** responsible for enabling or disabling the Rakurai scheduler.

Each validator must create a **Rakurai Activation Account (RAA)** — a PDA jointly controlled by both the validator and Rakurai. Enabling the Rakurai scheduler requires **2/2 multisig approval** (both validator and Rakurai). Disabling the scheduler can be done **unilaterally by either party (1/2 multisig)**. Validators can also configure the percentage of block rewards they want to retain (0–100%). The remaining portion is distributed to stakers.

- **[Initialize RAA](#31-init)**
- **[Enable or disable the scheduler](#32-scheduler-control)**
- **[Update commission](#33-update-commission)**
- **[Display RAA state](#34-show)**

For build and install steps, see the [CLI overview](./README.md#2-installation).

---

## 2. Usage

```sh
rakurai-activation [OPTIONS] --program-id <PROGRAM_ID> <COMMAND>
```

### 2.1. Global options

These options are **critical** and must be used with care:

- `-k, --keypair <PATH>`: Path to the Solana keypair file. This keypair must have authority to send transactions—typically the **validator identity keypair**.
- `-u, --url <URL>`: RPC URL of the target Solana cluster or moniker (for the testnet cluster, use `-u t` or `-u https://api.testnet.solana.com`).
- `-p, --program-id <PROGRAM_ID>`: Deployed Rakurai Activation program ID.
  - **Mainnet:** `rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi`
  - **Testnet:** `pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt`

> Incorrect keypair, RPC, or program ID will result in failed transactions.

---

## 3. Commands

### 3.1. `init`

#### 3.1.1. Description

To create a Rakurai Activation Account (RAA), you must provide your validator's `vote_pubkey` along with its corresponding *node identity* `keypair` as the signer. This node identity is used later to authorize or modify RAA parameters.

You can also specify a validator commission percentage on block rewards (`block_reward_commission_bps`) in basis points (BPS). The default is 100% (10,000 BPS), meaning the validator keeps the full block reward. If you set it below 100%, the remaining portion of the block reward is distributed to stakers, proportional to their stake, by the Reward Distribution program. This commission is **independent** of Solana's voting commission and applies only to block rewards.

Creating an RAA is a **mandatory step** for running a Rakurai-Solana validator.

#### 3.1.2. Usage

```sh
rakurai-activation -p <PROGRAM_ID> init --vote_pubkey <VOTE_PUBKEY> --keypair <IDENTITY_KEYPAIR> --url <RPC_URL>
```

#### 3.1.3. Arguments

- `-v, --vote_pubkey <PUBKEY>`: Validator's vote account public key.
- `-c, --block_reward_commission_bps <VALUE>` (optional): Validator's commission on block rewards in basis points (e.g., `500` for `5%`).

---

### 3.2. `scheduler-control`

#### 3.2.1. Description

Controls the Rakurai scheduler by enabling or disabling it.

- To **enable** the scheduler, both the validator and Rakurai must independently submit a transaction. The scheduler becomes active **only after both sides** have performed the enable transaction.
- To **disable** the scheduler, **either party** (the validator or Rakurai) can submit a disable transaction. Only one side is required to disable it.
- By default, this command **enables** the scheduler. To explicitly disable it, use the `-d` or `--disable_scheduler` flag.

#### 3.2.2. Usage

```sh
rakurai-activation -p <PROGRAM_ID> scheduler-control --identity_pubkey <IDENTITY_PUBKEY> --keypair <IDENTITY_KEYPAIR> --url <RPC_URL>
```

#### 3.2.3. Arguments

- `-d, --disable_scheduler`: Flag to disable the scheduler (default: enable).
- `-i, --identity_pubkey <PUBKEY>`: Validator identity account pubkey.

---

### 3.3. `update-commission`

#### 3.3.1. Description

Updates the validator's block reward commission.

Validators can update their share of the block reward at any time, independent of Rakurai. Only the validator can change their commission. The updated commission applies from the **current epoch** if no [RCA](../programs/reward_distribution/README.md#2-rca--block-rewards-for-stakers) has been opened yet, or from the **next epoch** if one already exists.

#### 3.3.2. Usage

```sh
rakurai-activation -p <PROGRAM_ID> update-commission --block_reward_commission_bps <VALUE> --identity_pubkey <IDENTITY_PUBKEY> --keypair <IDENTITY_KEYPAIR> --url <RPC_URL>
```

#### 3.3.3. Arguments

- `-c, --block_reward_commission_bps <VALUE>`: Commission percentage on block rewards in basis points (e.g., `500` means `5%`).
- `-i, --identity_pubkey <PUBKEY>`: Validator identity account pubkey.

---

### 3.4. `show`

#### 3.4.1. Description

Displays the current state of the Rakurai Activation Account (RAA).

- **Scheduler status**: Whether the Rakurai scheduler is enabled or disabled.
- **Validator commission**: The current block reward commission in basis points.
- **Authority**: The account authorized to manage the Rakurai Activation Account (RAA).

This command provides an overview of the RAA's current configuration and state.

#### 3.4.2. Usage

```sh
rakurai-activation -p <PROGRAM_ID> show --identity_pubkey <IDENTITY_PUBKEY> --url <RPC_URL>
```

#### 3.4.3. Arguments

- `-i, --identity_pubkey <PUBKEY>`: Validator identity account pubkey.
