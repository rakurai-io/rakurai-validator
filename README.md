# Rakurai-Solana Node Operator Guide

Welcome to the official guide for setting up and running a Rakurai-Solana node. This guide is intended for node operators who want to run Rakurai's high-block-reward client and maximize their rewards through efficient transaction scheduling.

The Rakurai Validator node is designed to maximize block rewards by leveraging advanced transaction scheduling techniques. This approach not only enhances Transactions Per Second (TPS) but also optimizes Compute Units (CU), leading to superior network performance. By efficiently processing high-reward transactions, node operators can significantly increase their earnings.
- [Rakurai-Solana Node Operator Guide](#rakurai-solana-node-operator-guide)
  - [1. Background](#1-background)
    - [What is Rakurai-Solana?](#what-is-rakurai-solana)
    - [High-Level Flow Architecture](#high-level-flow-architecture)
    - [Validator Incentives and Rewards Flow](#validator-incentives-and-rewards-flow)
    - [How Rakurai Interacts with the Solana Ecosystem](#how-rakurai-interacts-with-the-solana-ecosystem)
  - [2. QuickStart Guide](#2-quickstart-guide)
  - [3. Rakurai Smart Contracts (Programs)](#3-rakurai-smart-contracts-programs)
    - [Overview of Key Programs](#overview-of-key-programs)
    - [How the Programs Coordinate](#how-the-programs-coordinate)
    - [Deployed Program ID](#deployed-program-id)
  - [4. Setup and Build Rakurai Solana](#4-setup-and-build-rakurai-solana)
    - [Prerequisites](#prerequisites)
    - [Download and Build Rakurai-Solana](#download-and-build-rakurai-solana)
      - [Step 1: Clone the Rakurai-Solana](#step-1-clone-the-rakurai-solana)
      - [Step 2: Create Rakurai Activation Account (RAA)](#step-2-create-rakurai-activation-account-raa)
      - [Step 3: Download Rakurai Scheduler Binary](#step-3-download-rakurai-scheduler-binary)
        - [1. Sign the Rakurai Activation Account](#1-sign-the-rakurai-activation-account)
        - [2. Get Available Versions](#2-get-available-versions)
        - [3. Download the Scheduler Binary](#3-download-the-scheduler-binary)
      - [Step 4: Build the Client](#step-4-build-the-client)
      - [Step 5: Add Additional CLI Args](#step-5-add-additional-cli-args)
        - [Mainnet Arguments](#mainnet-arguments)
        - [Testnet Arguments](#testnet-arguments)
- [Verify Scheduler Binary Attestation](#verify-scheduler-binary-attestation)

---

## 1. Background

### What is Rakurai-Solana?
Rakurai-Solana is a high-performance Solana validator node designed to achieve **superior block rewards** and **higher Transactions Per Second (TPS)**. It incorporates heuristics-based **transaction scheduling** and other optimization techniques to efficiently process high-value transactions, boosting both performance and profitability for node operators.

### High-Level Flow Architecture
The Rakurai node is composed of four main components:
1. **Rakurai Scheduler Library** – A scheduler optimized for selecting high-value transactions.
2. **Rakurai Agave Client** – A fork of the jito-solana client modified to run the Rakurai scheduler library.
3. **Rakurai Activation Program** – A smart contract that controls node participation and enables validators to run a Rakurai node.
4. **Reward Distribution Program** – A mechanism for distributing validator rewards to stakers using a permissionless, Merkle-root-based verification model.

### Validator Incentives and Rewards Flow
With Rakurai's advanced transaction scheduler, validators can capture higher block rewards while improving both TPS and CU utilization. Rakurai charges a small commission on the increased total block rewards. The remaining amount can be fully retained by the validator or partially shared with stakers. Distribution is  executed via a **configurable, trustless, merkle-root-based system**.

### How Rakurai Interacts with the Solana Ecosystem
Rakurai nodes function like standard Solana validators but include performance enhancements focused on transaction throughput and block reward optimization. They remain fully compatible with the Solana protocol while offering measurable improvements in validator economics. Rakurai actively maintains and updates the scheduler library to ensure compatibility with the latest Solana releases.

---

## 2. QuickStart Guide

If you are already running Rakurai and only want to upgrade your node, Following are quick steps to get up and running with this release.

Note: If you are new to running Rakurai please follow the complete guide starting [here](#download-and-build-rakurai-solana)
- Download the [scheduler binary](#step-3-download-rakurai-scheduler-binary) authenticating it with your identity-key.
- Please make sure to update your submodules in this release using the following command:
    `git submodule update --init`
- Make sure to replace your old binary with new one in the correct paths.
- [Build and run](#step-4-build-the-client) your validator incorporating the scheduler binary you just downloaded.
- Please make sure you add --bam-url  in your startup script according to your [location](https://bam.dev/validators/#:~:text=Step%203%3A%20Choose%20Your%20Region).

Please share the output of this command after your first leader turn on Rakurai validator.
```bash
date; for p in "block time" "Banking packet delay" "rakurai_status"; do line=$(grep "$p" <LOG_FILE> | tail -n1); echo "$p: ${line:-not found}"; done
```
Replace <LOG_FILE> with your actual log file name.

---

## 3. Rakurai Smart Contracts (Programs)

### Overview of Key Programs

**rakurai_activation**: A multisig-based smart contract responsible for enabling or disabling the Rakurai scheduler. Each validator must create a `rakurai_activation_account (RAA)` (one-time setup), which is a multisig PDA controlled by both the validator and Rakurai. Both parties (2/2) must approve enabling the Rakurai scheduler—otherwise, the standard scheduler will run. However, either party can disable it unilaterally (1/2). Validators can also configure the percentage of block rewards they want to keep (0–100%). Any remaining rewards will be distributed to the stakers.

**rewards_distribution**: Similar in concept to the Jito tip distribution program, this contract handles the distribution of block rewards. For each epoch, an account is created that holds Merkle-root-based proofs of stake rewards. At the epoch’s end, rewards are distributed permissionlessly and verifiably. The validator's share of block rewards is never moved—it remains in the validator’s identity account.

### How the Programs Coordinate
Validators must first create a `rakurai_activation` account using the provided CLI. Once the account is active, they can download and run the Rakurai scheduler binary. Every epoch, a `rewards_distribution` account is created automatically to facilitate the staker rewards distribution.

### Deployed Program ID
- **Testnet**: Recommended for staging and integration testing.
   - *rakurai_activation*: `pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt`
   - *reward_distribution*: `A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB`
- **Mainnet**: Production environment
   - *rakurai_activation*: `rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi`
   - *reward_distribution*: `RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB`

---

## 4. Setup and Build Rakurai Solana

### Prerequisites

Ensure you have **Rust**, **Cargo**, and the **Solana CLI** installed before proceeding.

1. **Rust and Cargo**: [Installing Rust and Cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html#install-rust-and-cargo)
2. **Solana CLI**: [Solana CLI Installation](https://solana.com/docs/intro/installation)
3. **Anchor**: [Installing Anchor](https://solana.com/docs/intro/installation#install-anchor-cli) (optional)

Additionally:
- Keep system packages updated (e.g., `sudo apt update && sudo apt upgrade`)
- Refer to the [Solana Validator Setup Guide](https://docs.anza.xyz/operations/guides/validator-start) for Solana's official documentation.
---

### Download and Build Rakurai-Solana

#### Step 1: Clone the Rakurai-Solana
Clone the latest Rakurai-Solana release with submodules:
```bash
git clone https://github.com/rakurai-io/rakurai-validator.git --recurse-submodules -b <version>-rakurai 
cd ./rakurai-validator
git checkout release/<version>-rakurai

          OR

# Steps if you have the repo cloned already
git fetch
git checkout release/<version>-rakurai
# If you are on a previous branch where rakurai_scheduler was addeded as a submodule then run the following command before updating the submodules
git rm --cached core/src/banking_stage/rakurai_scheduler
git submodule update --init --recursive
```

Export the Rakurai CLI path:
```bash
echo "export PATH=\"$(pwd)/rakurai_programs/release/downloads:\$PATH\"" >> ~/.bashrc && source ~/.bashrc
```

#### Step 2: Create Rakurai Activation Account (RAA)
Use the CLI to initialize your validator's [activation account](#overview-of-key-programs). The following command will return a Pubkey (`RAKURAI_ACTIVATION_ACCOUNT_PUBKEY`), which will be used in the [next step](#step-2-download-scheduler-binary)

> **Note**: 
 - If you already have created rakurai activation account then run `rakurai-activation -p <PROGRAM_ID> show -i <IDENTITY_PUBKEY> -um` to get your <RAKURAI_ACTIVATION_ACCOUNT_PUBKEY>
 - You must create a separate RAA for **each cluster** you participate in (e.g., **testnet**, **mainnet-beta**). Each cluster has its own Rakurai activation program.

```bash
rakurai-activation -p <PROGRAM_ID> init \
 --vote_pubkey <VOTE_PUBKEY> \
 --keypair <IDENTITY_KEYPAIR> \
 --url <RPC_URL>
```

Arguments:
- `--program-id <PROGRAM_ID>`: Specify the Rakurai Activation Program ID.
    - Mainnet: `rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi`
    - Testnet: `pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt`
- `--vote_pubkey <VOTE_PUBKEY>`: Validator vote account public key.
- `--keypair <IDENTITY_KEYPAIR>`: Path to validator identity keypair file.

Optional Argument:
- `--block_reward_commission_bps <VALUE>`: Validator commission percentage on block rewards in basis points (i.e 100 bps = 1%). [Default Value: 10000 bps]

> More CLI details are available in the Rakurai [CLI Documentation](https://github.com/rakurai-io/rakurai_programs/blob/release/v0.1.2/cli/README.md).

#### Step 3: Download Rakurai Scheduler Binary

Before running the scheduler, you must **authenticate** and download the correct binary for your OS and release version.

---

##### 1. Sign the Rakurai Activation Account

Use your **validator identity keypair** to sign the activation account’s public key:

```bash
solana sign-offchain-message <RAKURAI_ACTIVATION_ACCOUNT_PUBKEY> \
  --keypair /path/to/validator-keypair.json
```

This command will output a base58-encoded `SIGNATURE`. You’ll use this in the next step to verify your request.

---

##### 2. Get Available Versions

You **must fetch the available scheduler versions** and **match your OS exactly** (e.g., `ubuntu_24.04`):

```bash
curl -X GET https://api.rakurai.io/api/v1/scheduler/versions
```

Example response:

```json
[
  {
    "os": "ubuntu_24.04",
    "mainnet_and_testnet": ["v2.3.6-rakurai.0"],
    "testnet_only": []
  }
]
```
---

##### 3. Download the Scheduler Binary

Using the signature from step 1 and the version from step 2, download the binary:

```bash
curl -o rakurai-scheduler.tar.gz https://api.rakurai.io/api/v1/downloads/scheduler \
  -H "Content-Type: application/json" \
  -d '{
    "activation_account": "<RAKURAI_ACTIVATION_ACCOUNT_PUBKEY>",
    "signature": "<SIGNATURE>",
    "version": "<VERSION>",
    "os": "<OS_VERSION>"
  }' \
  --fail-with-body || cat rakurai-scheduler.tar.gz
```

> Ensure you:
> - Match `version` **exactly** from the `/scheduler/versions` API.
> - Use the **correct OS key** (`ubuntu_24.04`, `ubuntu_22.04`, etc).
> - Replace `activation_account` and `signature` with valid values.

---
You can also download the scheduler library from [Rakurai.io](https://rakurai.io/validators#).

#### Step 4: Build the Client

In the root folder of the repository, run the following commands:
```bash
# Extract the Rakurai scheduler library
mkdir -p ./target/release/
tar -xvzf ./rakurai-scheduler.tar.gz
# It will extract itself into a folder where binaries for different OS versions are present
# copy the version according to your OS version
cp librak*.so ./target/release/

# Build the client
cargo build --release --features build_validator

# Export the scheduler binary path
export LD_LIBRARY_PATH=$LD_LIBRARY_PATH:<path_to_rakurai-validator>/target/release
```
- *`Note`*: If you're using a custom launch script like `validator.sh`, make sure to export the scheduler binary path within it.

#### Step 5: Add Additional CLI Args
Modify your validator startup script by appending the following arguments:
##### Mainnet Arguments
```bash
 --rewards-merkle-root-authority H21wFgN53ghjDq5N9QhraAiPn1tRVYkobySj55unXLEj \
 --rakurai-activation-program-id rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi \
 --reward-distribution-program-id RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB
```
##### Testnet Arguments
```bash
 --rewards-merkle-root-authority H21wFgN53ghjDq5N9QhraAiPn1tRVYkobySj55unXLEj \
 --rakurai-activation-program-id pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt \
 --reward-distribution-program-id A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB
```

You can add the following argument in your startup script to specify the validator client's mode of operation.
It will default to `rakurai-jito` if not specified. Other available option is `bam-strict-compliance`. `rakurai-bam` is restricted until complete compatability with BAM is ensured. 

```bash
 --client-mode <CLIENT_MODE>
```

You can change the client-mode through admin RPC without restarting your validator using the following command

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"setClientMode","params":["<CLIENT_MODE>"]}' | socat - UNIX-CONNECT:admin.rpc
```

You can **optionally** add the following argument to delay QUIC packets that are not coming from the Jito relayer.
This emulates the approach used by Jito in their relayer. It will default to `200` if not specified.

```bash
 --banking-packet-delay-ms <BANKING_PACKET_DELAY_MS>
```

There is another **optional** argument available to adjust the block times within the protocol limits. The default value of this argument is set to 10 which translates to 390ms block times. You can set it to a max of 50 which translates to 350ms

```bash
 --target-slot-adjustment-ms <TARGET_SLOT_ADJUSTMENT_MS>
```

  - [**Agave Validator arguments**](https://docs.anza.xyz/operations/setup-a-validator#create-a-validator-startup-script)  
  - [**Jito-Solana arguments**](https://jito-foundation.gitbook.io/mev/jito-solana/command-line-arguments)  

*Important*: 
 - If you set `--rewards-merkle-root-authority` to `H21wFgN53ghjDq5N9QhraAiPn1tRVYkobySj55unXLEj`, Rakurai will automatically distribute rewards to your stakers using the reward distribution program. 
 - If you set it to any other address, you will need to run the claim workflow manually.

# Verify Scheduler Binary Attestation

The Rakurai scheduler binary comes with [GitHub artifact attestation](https://docs.github.com/en/actions/concepts/security/artifact-attestations), providing cryptographic proof of its build provenance and integrity. You can validate the scheduler binary's attestations (provenance and SBOM) and checksum by following the detailed instructions in the [Attestation Guide](./VERIFY-RAKURAI-BINARY.md).

---
