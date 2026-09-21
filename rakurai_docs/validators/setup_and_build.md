# Rakurai Setup — Guide

**This is the one-time work that switches your validator onto Rakurai.** At the end of it you are running the Rakurai client, you have an on-chain activation account that controls your participation and your commission, and your node starts receiving TIN orderflow.

You build the client, install the prebuilt Rakurai scheduler library, create your Rakurai Activation Account (RAA), and restart with a few extra CLI arguments. Two settings are worth deciding before you start: your **block reward commission** — how much you keep versus share with stakers — and whether you delegate Merkle root authority to Rakurai so staker distribution runs automatically at 0% fee.

**Audience:** Validator operators setting up Rakurai for the first time or performing a full reinstall.

---

## 1. Prerequisites

Ensure you have **Rust**, **Cargo**, and the **Solana CLI** installed before proceeding.

1. **Rust and Cargo:** [Installing Rust and Cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html#install-rust-and-cargo)
2. **Solana CLI:** [Solana CLI Installation](https://solana.com/docs/intro/installation)
3. **Anchor:** [Installing Anchor](https://solana.com/docs/intro/installation#install-anchor-cli) (optional)

Additionally:

- Refer to the [Solana Validator Setup Guide](https://docs.anza.xyz/operations/guides/validator-start) for Solana's official documentation.

---

## 2. Download and build Rakurai-Solana

### 2.1. Clone the Rakurai-Solana repository

Clone the latest Rakurai-Solana release with submodules:

```bash
git clone https://github.com/rakurai-io/rakurai-validator.git --recurse-submodules
cd ./rakurai-validator
git checkout <RELEASE_TAG>
```

Or, if you already have the repo cloned:

```bash
git fetch
git checkout <RELEASE_TAG>
# If you are on a previous branch where rakurai_scheduler was added as a submodule,
# run the following command before updating submodules:
git rm --cached core/src/banking_stage/rakurai_scheduler
git submodule update --init --recursive
```

Export the Rakurai CLI path:

```bash
echo "export PATH=\"$(pwd)/rakurai_programs/release/downloads:\$PATH\"" >> ~/.bashrc && source ~/.bashrc
```

### 2.2. Create Rakurai Activation Account (RAA)

Use the CLI to initialize your validator's [activation account](../rakurai_programs/programs/rakurai_activation/README.md). The following command returns a Pubkey (`RAKURAI_ACTIVATION_ACCOUNT_PUBKEY`), which is used when [downloading the scheduler binary](#23-download-rakurai-scheduler-binary).

> [!WARNING]
> **One RAA per validator, per cluster**
>
> An RAA is uniquely tied to a **validator identity**, and each cluster uses a **different Rakurai Activation Program ID**. Create a separate RAA for every validator *and* every cluster (testnet and mainnet-beta are distinct accounts).
>
> If you already created one, do not initialize again — recover the pubkey with:
>
> ```bash
> rakurai-activation -p <PROGRAM_ID> show -i <IDENTITY_PUBKEY> -um
> ```

```bash
rakurai-activation -p <PROGRAM_ID> init \
  --vote_pubkey <VOTE_PUBKEY> \
  --keypair <IDENTITY_KEYPAIR> \
  --url <RPC_URL>
```

Arguments:

- `--program-id <PROGRAM_ID>`: Rakurai Activation Program ID.
  - Mainnet: `rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi`
  - Testnet: `pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt`
- `--vote_pubkey <VOTE_PUBKEY>`: Validator vote account public key.
- `--keypair <IDENTITY_KEYPAIR>`: Path to validator identity keypair file.

Optional argument:

- `--block_reward_commission_bps <VALUE>`: Validator commission percentage on block rewards in basis points (100 bps = 1%). Default: 10000 bps.

For more details, refer to the [latest release](https://github.com/rakurai-io/rakurai_programs/releases/latest) of the Rakurai Activation CLI and the [Activation program guide](../rakurai_programs/programs/rakurai_activation/README.md).

### 2.3. Download Rakurai Scheduler Binary

Before running the scheduler, you must **authenticate** and download the correct binary for your OS and release version.

The download is authenticated per validator, but the artifact you receive is not.

> [!NOTE]
> **The binary is not tied to a validator**
>
> Your [RAA](#22-create-rakurai-activation-account-raa) is tied to a validator identity and is what authenticates the download. The **scheduler binary itself is not** — you can reuse the same binary across multiple validators running the same release and OS.

#### 2.3.1. Sign the Rakurai Activation Account

Use your **validator identity keypair** to sign the activation account's public key:

```bash
solana sign-offchain-message <RAKURAI_ACTIVATION_ACCOUNT_PUBKEY> \
  --keypair /path/to/validator-keypair.json
```

This command outputs a base58-encoded `SIGNATURE`. Use it in the next step to verify your request.

#### 2.3.2. Get available versions

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

#### 2.3.3. Download the scheduler binary

Using the signature from step 2.3.1 and the version from step 2.3.2, download the binary:

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

> [!WARNING]
> **Check these before downloading**
>
> - Match `version` **exactly** from the `/scheduler/versions` API.
> - Use the **correct OS key** (`ubuntu_24.04`, `ubuntu_22.04`, etc.).
> - Replace `activation_account` and `signature` with valid values.

After download, verify the binary with [Binary attestation](./binary_attestation.md).

---

## 3. Build the client

In the root folder of the repository, run:

```bash
# Extract the Rakurai scheduler library
mkdir -p ./target/release/
tar -xvzf ./rakurai-scheduler.tar.gz
# It will extract into a folder where binaries for different OS versions are present.
# Copy the version according to your OS version:
cp librak*.so ./target/release/

# Build the client
cargo build --release --features build_validator

# Export the scheduler binary path
export LD_LIBRARY_PATH=$LD_LIBRARY_PATH:<path_to_rakurai-validator>/target/release
```

> [!WARNING]
> **LD_LIBRARY_PATH must be set where the validator starts**
>
> Exporting `LD_LIBRARY_PATH` in your interactive shell is not enough. If you launch through a custom script such as `validator.sh`, or through a systemd unit, export the scheduler library path **inside that script or unit** — otherwise the validator starts without finding `librak*.so`.

---

## 4. Grant capabilities for XDP (Linux-only)

XDP transmit is enabled on Linux by default and requires extra capabilities. After building, grant them to the validator binary:

> [!WARNING]
> **Re-run setcap after every rebuild**
>
> Capabilities are attached to the binary file, not to the path. `cargo build --release` writes a new binary and **drops the capabilities**, so you must run `setcap` again after every rebuild or upgrade.

```bash
$ sudo setcap 'cap_net_admin,cap_net_raw+eip' <path-to-agave-validator-binary>
```

For XDP zero-copy mode (`--xdp-zero-copy`), additional capabilities are needed:

```bash
$ sudo setcap 'cap_net_admin,cap_net_raw,cap_bpf,cap_perfmon+eip' <path-to-agave-validator-binary>
```

---

## 5. Add additional CLI args

Modify your validator startup script by appending the following arguments.

### 5.1. Mainnet arguments

```bash
 --rewards-merkle-root-authority H21wFgN53ghjDq5N9QhraAiPn1tRVYkobySj55unXLEj \
 --rakurai-activation-program-id rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi \
 --reward-distribution-program-id RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB \
 --rakurai-tip-manager-program-id rKtiPTD7WuCdEEQ2JXWgAmZHHL9iZLc3niCXwtS7wSH
```

### 5.2. Testnet arguments

```bash
 --rewards-merkle-root-authority H21wFgN53ghjDq5N9QhraAiPn1tRVYkobySj55unXLEj \
 --rakurai-activation-program-id pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt \
 --reward-distribution-program-id A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB \
 --rakurai-tip-manager-program-id 4qRZaFzf7MvgfBTCP9grb69cCST8UmKHPtkpGAgkJosD
```

### 5.3. Optional slot adjustment

An **optional** argument adjusts block times within protocol limits. The default value is `10` (390 ms block times). You can set it to a maximum of `50` (350 ms):

```bash
 --target-slot-adjustment-ms <TARGET_SLOT_ADJUSTMENT_MS>
```

Reference:

- [Agave Validator arguments](https://docs.anza.xyz/operations/setup-a-validator#create-a-validator-startup-script)
- [Jito-Solana arguments](https://jito-foundation.gitbook.io/mev/jito-solana/command-line-arguments)

> [!TIP]
> **Delegating the Merkle root authority automates distribution**
>
> Setting `--rewards-merkle-root-authority` to `H21wFgN53ghjDq5N9QhraAiPn1tRVYkobySj55unXLEj` lets Rakurai snapshot, build the Merkle tree, upload the root, and run staker claims for you at **0% distribution fee**, using the [Reward Distribution RCA](../rakurai_programs/programs/reward_distribution/README.md#3-rca--block-rewards-for-stakers).
>
> Set it to any other address and you own the whole claim workflow yourself — see [Block Reward Distribution](../../block-rewards-distributor/block_reward_distribution.md).
