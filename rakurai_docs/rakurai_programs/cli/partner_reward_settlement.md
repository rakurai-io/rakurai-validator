# Partner Tip and MevShare Revenue Settlement CLI (`rakurai-revshare`)

**This is how partners pay validators what they owe at the end of each epoch.** Two revenue kinds, two different workflows: a custom **tip** account is recorded on-chain for you every leader turn and you simply transfer, whereas a **MevShare** backrun share is invisible to the chain and must be *reported* by you first, then transferred.

Both the workflow and the `--revenue-kind` flag matter — the wrong flag targets the wrong vault, and reporting twice books twice what you owe. Read [§1](#1-what-this-cli-is-about) before running anything that moves SOL.

**Audience:** Transaction-landing services and post-pack partners.

Install: [CLI overview](./README.md#2-installation). Prepaid P2C subscription (Users/Consumers): [`rakurai-p2c`](./p2c_subscription.md). Program details: [Reward Distribution](../programs/reward_distribution/README.md).

---

## 1. What this CLI is about

Partners can keep custody of collected SOL in their own accounts (custom tips and/or post-pack MevShare). Rakurai **cannot drain** those accounts, so after each epoch you must settle the agreed share into the validator’s on-chain vault.

This CLI is how you do that:

- **Read** TCA / MCA vaults (`get-account`, `get-all-accounts`, pending-record commands)
- **Record** MCA MevShare for the current epoch (`record-revenue`)
- **Settle** by transferring SOL into the vault (`transfer`, `transfer-all`)

Claim/distribution of settled funds is the **manager** flow, not this CLI. Partner MCA / TCA vaults are created by Rakurai / ops before you settle.

`--revenue-name` is the service id Rakurai assigned when you enable custom tips and/or post-pack. It is a PDA seed — use the exact value.

### 1.1. When to use `--revenue-kind Tip` vs `Mev-share`

Pass `--revenue-kind` on read / record / transfer commands. The two kinds are **different vaults**.

| | `--revenue-kind Tip` (TCA) | `--revenue-kind Mev-share` (MCA) |
| --- | --- | --- |
| Use when | You run a **custom tip account**. Tips land in your account; the validator records the owed share each leader turn. | You share **post-pack / backrun (MevShare)** revenue. Nothing is recorded during the epoch. |
| After the epoch | **Transfer only** | **Record, then transfer** |
| Who records | Validator (automatic) | You, with the MCA `record_authority` keypair |

> [!CAUTION]
> **Settle within 2 epochs**
>
> If a service does not record and settle within **2 epochs**, post-pack access and MCA prioritization stop after that grace period. For custom tip accounts, the same delay stops the tip account from being used for prioritization.

---

## 2. What you do each epoch

### 2.1. Custom tips — `--revenue-kind Tip`

1. Wait for the epoch to end.
2. `get-account` / `get-all-accounts` — check balance, total owed, deficit (if any).
3. If epochs are pending, settle: `transfer` or `transfer-all`.
4. Keep the transaction signature.

### 2.2. Post-pack MevShare — `--revenue-kind Mev-share`

1. Wait for the epoch to end.
2. `record-revenue` once per validator (MCA `record_authority` keypair).
3. `get-account` / `get-all-accounts` — confirm pending amount.
4. Settle: `transfer` or `transfer-all`.
5. Keep the transaction signatures.

---

## 3. Commands

Global flags: `-u` / `--url`, `-p` / `--program-id`, `-k` / `--keypair`.

### 3.1. `get-account`

One vault for one validator. Summary: pubkey, name, vote, balance after rent-exempt, total owed (past epochs only — current cluster epoch excluded), and **deficit only if greater than zero**. An underfunded alert appears only when balance is below owed. `--detail` adds record authority and per-epoch rows (sorted by epoch).

```sh
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-account \
  --revenue-kind Mev-share \
  --revenue-name <REVENUE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY>
```

### 3.2. `get-all-accounts`

All vaults for `--revenue-kind` + `--revenue-name`. Default: vote × epoch pending table. `--detail` adds per-account fields.

### 3.3. `get-pending-record` / `get-all-pending-records`

Inspect one epoch or every unsettled epoch on one vault.

### 3.4. `record-revenue` (MCA only)

Requires `--revenue-kind Mev-share` and the MCA `record_authority`. Ledger only — **no SOL moves**. Current cluster epoch.

> [!WARNING]
> **Record once per validator per epoch**
>
> `record-revenue` **adds** to the epoch entry rather than overwriting it. Running it twice for the same epoch and vote records twice the amount owed, and you will then be asked to transfer that inflated figure. Confirm the current entry with `get-pending-record` before recording.

```sh
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <RECORD_AUTHORITY_KEYPAIR> \
  record-revenue \
  --revenue-kind Mev-share \
  --revenue-name <REVENUE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY> \
  --amount <LAMPORTS>
```

### 3.5. `transfer` / `transfer-all`

Settle one epoch on one vault, or all pending epochs across matching vaults. `--dry-run` supported.

---

## Appendix: Program IDs

| Cluster | Reward Distribution | Rakurai Activation |
| ------- | ------------------- | ------------------ |
| Mainnet | `RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB` | `rAKACC6Qw8HYa87ntGPRbfYEMnK2D9JVLsmZaKPpMmi` |
| Testnet | `A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB` | `pmQHMpnpA534JmxEdwY3ADfwDBFmy5my3CeutHM2QTt` |

> [!WARNING]
> **These commands move real SOL**
>
> Verify the RPC cluster, program ID, vote pubkey, revenue name, epoch, and lamport amount before recording or transferring. `transfer` and `transfer-all` accept `--dry-run` — use it first.
