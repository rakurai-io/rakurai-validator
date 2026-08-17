# Partner Tip and MevShare Revenue Settlement CLI

Inspect, record (MCA), and settle custom tip and post-pack revenue in validator TCA/MCA vaults.

**Audience:** Transaction-landing services and post-pack partners.

**Product:** Partner Tip and MevShare Revenue Settlement CLI  

For install, see the [CLI overview](./README.md#2-installation). Program details: [Tip and MevShare accounts](../programs/reward_distribution/README.md#5-tip-and-mevshare-collection-accounts).

---

## 1. When to use this CLI

Use this CLI if you are a **transaction-landing service** or **post-pack** partner that holds tip or MevShare revenue in an account **Rakurai cannot drain**, and you must settle the agreed share into the validator’s on-chain vault after the epoch.

After the epoch ends, the usual flow is:

```sh
# 1. See pending shares across all validators
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-all-accounts \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME>

# 2. Settle everything pending (dry-run first if you like)
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <PARTNER_PAYER_KEYPAIR> \
  transfer-all \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME>
```

Swap `--revenue-kind` to `Mev-share` for post-pack MCA settlement. **MCA** also needs `record-revenue` per validator/epoch before `transfer-all`. For flags, dry-run, batch size, single-vault transfer, and other details, see the [Commands](#3-commands) subcommands below.

- **TCA (Tips Collection Account)** — `--revenue-kind Tip`  
  Use when you run a **custom tip account**. Tips land in your account; the **validator records** the owed share in the TCA each leader turn. After the epoch, you only **transfer** that amount into the TCA.

- **MCA (MevShare Collection Account)** — `--revenue-kind Mev-share`  
  Use when you share **post-pack** revenue. Nothing is recorded during leader turns. After the epoch, **you** must **record** the owed amount once on the MCA, then **transfer** SOL into the MCA.

### MCA setup (post-pack)

When you start using **post-pack**, Rakurai creates an **MCA** for your service (one per service per validator). That MCA is where you:

1. **Record** the MevShare revenue owed for the epoch (`record-revenue`)
2. **Transfer** the corresponding SOL into the MCA (`transfer`)

You **must hold the MCA `record_authority`** keypair. Only that authority can call `record-revenue`. Confirm it with `get-account` (the `Record auth` field). Keep that keypair safe — without it you cannot update the MCA ledger.

Do **not** use this CLI for Rakurai’s own eight tip PDAs — those are drained by the Tip Manager. This CLI is only for **partner-owned** custom tip TCAs and MevShare MCAs.

### Revenue name (`--revenue-name`)

`--revenue-name` is the unique service id for your integration (similar to a UUID). Rakurai assigns it when you connect with the team and enable **custom tip accounts** and/or **post-pack**. It is a PDA seed for your TCA/MCA — use the exact value you were given; a different name derives a different vault.

---

## 2. Why settlement is required

Partners may register a custom tip account (or post-pack flow) so their transactions get scheduler priority while they keep custody of the collected SOL. Because Rakurai cannot drain that account, the partner must settle the agreed revenue share after the epoch.

**TCA:** the validator already wrote the ledger during leader turns — you only settle SOL (`transfer`).

**MCA:** the validator does **not** write the ledger during the epoch. Post-pack MevShare stays in your own accounts until you:

1. **Record** the owed share on-chain (`record-revenue`) — ledger only; no lamports move.
2. **Settle** by transferring SOL into the MCA (`transfer`).
3. Claim/distribution is handled by the configured manager (not this CLI).

See [Post-epoch stage: record and settle](../../transaction_inclusion/post_pack_confirmations.md#44-post-epoch-stage-record-and-settle).

> **Note:** If a service does not record and settle within 2 epochs, post-pack access and MCA prioritization stop after a two-epoch grace period.

---

## 3. Commands

### 3.1. `get-account`

Derives and fetches **one** partner revenue vault for a specific validator.

Required: `--revenue-kind`, `--revenue-name`, `--vote-pubkey`.  
Default: `--account-version auto` (prefer V1, else legacy).

```sh
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-account \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY>
```

Output: derived address, layout (`legacy` / `v1`), type (`Tip` / `Mev-share`), revenue name, vote pubkey, record authority, balance.

### 3.2. `get-all-accounts`

Lists every TCA/MCA for your service across validators (no `--vote-pubkey`). RPC scan by `--revenue-kind` + `--revenue-name`.

**Default:** a pivot table of **pending** amounts — rows = validators (vote), columns = epochs. Epoch cells and row TOTAL use **lamports** (row TOTAL also shows SOL to 5 decimals). The footer TOTAL row is **SOL only** (5 decimals). A `Unit:` line above the table states this.

```sh
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-all-accounts \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME>
```

**`--detail`:** after the table, print per-account fields (pubkey, layout, vote, record authority, balance, and pending by epoch).

```sh
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-all-accounts \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME> \
  --detail
```

Use `--revenue-kind Mev-share` for MCAs.

### 3.3. `get-pending-record`

Reads the record for one epoch:

```sh
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-pending-record \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY> \
  --epoch <EPOCH>
```

- **V1:** `pending_amount = recorded_amount - transferred_amount`
- **Legacy:** no `transferred_amount`; `pending_amount` is the unclaimed recorded amount (not proof a prior SOL transfer did not occur)

### 3.4. `get-all-pending-records`

Lists every epoch that still needs settlement **for one vault** (one vote pubkey):

```sh
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-all-pending-records \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY>
```

Use `--revenue-kind Mev-share` for MCA. Results are ordered by epoch.

### 3.5. `record-revenue` (MCA only)

Records MevShare revenue on the MCA ledger for the **current cluster epoch**. Calls `record_revenue` (legacy) or `record_revenue_v1` (V1). Updates the on-chain ledger only — **no SOL is moved**.

**Why it is required for MCA:** unlike TCA, the validator does not record MevShare during leader turns. Post-pack revenue stays in your accounts until you report the owed share once per epoch. Without `record-revenue`, there is nothing to settle and claim. See [post-epoch record and settle](../../transaction_inclusion/post_pack_confirmations.md#44-post-epoch-stage-record-and-settle).

Requires `--revenue-kind Mev-share`. When post-pack is enabled, Rakurai creates your MCA; you must hold its **`record_authority`** and pass that keypair as `--keypair` (shown by `get-account` as `Record auth`).

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

There is no `--epoch` flag: the program attributes the amount to `Clock`’s current epoch. Repeated calls for the same epoch accumulate.

### 3.6. `transfer`

Settles SOL for **one** existing epoch record on **one** validator vault:

```sh
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <PARTNER_PAYER_KEYPAIR> \
  transfer \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY> \
  --epoch <EPOCH> \
  --amount <LAMPORTS>
```

If `--amount` is omitted, transfers the full pending amount. Refuses zero, claimed, or over-pending amounts.

Pass `--dry-run` to preview vault/epoch/amount without sending a transaction.

### 3.7. `transfer-all`

Settles **all pending epochs on every vault** matching `--revenue-kind` + `--revenue-name` (all validators). Prints the same vote×epoch pending **table** as `get-all-accounts`, then sends settle/transfer instructions in batches (**10 per transaction** by default; override with `--batch-size`).

Legacy = system transfer; V1 = `settle_revenue` per epoch. Rakurai’s own tip TCA is skipped.

```sh
# Preview table only
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <PARTNER_PAYER_KEYPAIR> \
  transfer-all \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME> \
  --dry-run

# Settle everything pending (default 10 ix/txn)
rakurai-revshare \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <PARTNER_PAYER_KEYPAIR> \
  transfer-all \
  --revenue-kind Mev-share \
  --revenue-name <REVENUE_NAME>

# Smaller batches if RPC rejects large txs
rakurai-revshare \
  ... \
  transfer-all \
  --revenue-kind Tip \
  --revenue-name <REVENUE_NAME> \
  --batch-size 4
```

For MCA, record each epoch first (`record-revenue`) before `transfer-all`.

---

## 4. Legacy vs V1

| | Legacy | V1 |
| --- | --- | --- |
| Record (MCA) | `record_revenue` | `record_revenue_v1` |
| Transfer | System transfer into the vault | `settle_revenue` (transfer + credit `transferred_amount`) |
| Retry risk | Can double-fund if you re-run transfer without checking balance/signature | Ledger tracks what was already transferred |
| Force layout | `--account-version legacy` | `--account-version v1` |

Default `--account-version auto`: prefer V1 if it exists, else legacy; error if neither exists.

Rakurai’s own tip TCA is settled by Tip Manager — `settle_revenue` is blocked for that account. This CLI is for custom partner TCA/MCA only.

PDA derivation depends on `--revenue-name`, `--vote-pubkey`, `--revenue-kind`, program ID, and account version. Use the exact revenue name assigned by Rakurai.

See [dual vaults (legacy vs V1)](../programs/reward_distribution/README.md#58-dual-vaults-legacy-vs-v1).

---

## 5. Workflows

### 5.1. Custom tips (TCA)

1. Register a custom tip account and revenue-share percentage with Rakurai (you receive a `--revenue-name`) — see [Tips FAQ Q4](../../transaction_inclusion/rakurai_tip_manager_faqs.md#4-can-i-use-my-own-tip-account-instead-of-rakurais-eight-accounts).
2. Receive tips in your account during the epoch.
3. Confirm the TCA epoch record exists (validator recorded it).
4. Run `get-all-pending-records --revenue-kind Tip` (or `get-all-accounts` / `transfer-all` to cover every validator at once).
5. After the epoch ends, run `transfer --revenue-kind Tip` for one vault, or `transfer-all --revenue-kind Tip` for all vaults.
6. Keep the transaction signature for reconciliation.

### 5.2. Post-pack (MCA)

1. Complete post-pack / transaction-landing integration with Rakurai — Rakurai creates your MCA and assigns `--revenue-name` and `record_authority` (you must hold that keypair) — see [MEV revenue sharing](../../transaction_inclusion/post_pack_confirmations.md#4-mev-revenue-sharing).
2. After the epoch ends, run `record-revenue --revenue-kind Mev-share --amount <LAMPORTS>` with the MCA `record_authority` keypair (per validator) — see [post-epoch record and settle](../../transaction_inclusion/post_pack_confirmations.md#44-post-epoch-stage-record-and-settle).
3. Confirm with `get-all-accounts` / `get-all-pending-records --revenue-kind Mev-share`.
4. Run `transfer --revenue-kind Mev-share` for one vault, or `transfer-all --revenue-kind Mev-share` for all pending settlements.
5. Keep the transaction signatures for reconciliation.

This CLI does not claim/distribute settled funds (manager flow).

> **Note:** If a service does not record and settle within 2 epochs, post-pack access and MCA prioritization stop after a two-epoch grace period.

---

## Appendix: Program IDs

| Cluster | Reward Distribution program |
| ------- | --------------------------- |
| Mainnet | `RAkd1EJg45QQHeuXy7JEWBhdNvsd64Z5PbZJWQT96iB` |
| Testnet | `A37zgM34Q43gKAxBWQ9zSbQRRhjPqGK8jM49H7aWqNVB` |

Verify RPC cluster, program ID, vote pubkey, revenue name, epoch, and lamports before recording or transferring.
