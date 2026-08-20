# Rakurai Client Config Program

On-chain **scheduler configuration** for Rakurai validators. This program stores **configuration only**. It does not hold tips, MevShare vaults, or P2C subscription SOL. Those live in [Reward Distribution](../reward_distribution/README.md) (TCA / PSA / MCA).

➤ IDL: [rakurai_client_config.json](./idl/rakurai_client_config.json).

---

## 1. Deployed program ID

- **Current clusters:** [4uGNMjJFxgE3TfEiPmSpvfwYah12QZbaWWZDJqZvA9F4](https://solscan.io/account/4uGNMjJFxgE3TfEiPmSpvfwYah12QZbaWWZDJqZvA9F4)

---

## 2. What you configure

One versioned payload (`Config::V1` / `ConfigV1`) on every PDA. Three independent sections:

| Section | What it is | Entry shape |
|---------|------------|-------------|
| **`block_engine`** | Endpoints the scheduler **receives bundles from** | Named set → list of `{ url, max_bundles, period_ms, max_bundle_burst }` |
| **`p2c`** | Endpoints the scheduler **sends transactions to** for arbitrage / backrun | Named set → list of `{ url }` |
| **`virtual_priority`** | Tip accounts used to **virtually prioritize** transactions | Named set → list of `{ key: tip-account pubkey, value: percent of that tip }` |

A **set** has a 32-byte `name` (`Uuid`) plus its `url` list. The validator node reads **effective** config for a vote as **global ∪ validator**, merged **by set name** (validator wins on the same name).

### 2.1. Block engine

Block-engine URLs are where **transaction-landing services submit bundles**. The scheduler **connects out and receives bundles** from those endpoints (with per-URL quotas).

### 2.2. Post-pack (P2C)

Post-pack URLs are where the scheduler **sends transactions** so consumers can run **arbitrage / backrun** bundles. Updates are generated from the **point of no return** — consumers only see transactions just before they become part of the block, which **prevents front-running**. Product guide: [Post-pack confirmations](../../../transaction_inclusion/post_pack_confirmations.md).

This is endpoint config only. Prepaid P2C **subscription SOL** is a [PSA](../reward_distribution/README.md#4-psa--prepaid-fee-to-use-post-pack); backrun **revenue share** is an [MCA](../reward_distribution/README.md#5-mca--sharing-post-pack-backrun-profit).

### 2.3. Virtual priority

Virtual-priority entries name **tip accounts**. When a transaction (or bundle) tips that account, the scheduler uses a **configured percent of that tip amount** as extra virtual priority — so the txn can be ordered higher without changing the SOL that actually moved.

- `key` — tip-account pubkey (Rakurai tip PDA or a registered custom tip account)
- `value` — fraction of that tip used for virtual priority; must be in `[0.0, 1.0]` (for example `0.1` = 10% of the tip)

---

## 3. Full replace — current + new

Every write instruction takes a **complete `Config`**. The program does **not** patch, append, or merge with what is already on-chain.

To add a new block-engine, P2C, or virtual-priority set:

1. Read the **current** payload (`global show`, `validator show`, or `union --vote`)
2. Keep **every existing set** you still want
3. Add the **new** set (or add a URL / VP key inside an existing set)
4. Submit that **full JSON** (`update_global`, `update_validator`, or `proposal submit`)

Submitting a file that contains **only the new set** replaces the PDA contents with that file. Other sets on **that PDA** are dropped.

```text
# Wrong — wipes every other BE / P2C / VP set on this PDA
{ "block_engine": { "sets": [ { "name": "new-be", "url": [...] } ] }, "p2c": { "sets": [] }, "virtual_priority": { "sets": [] } }

# Right — current sets + the new one
{ "block_engine": { "sets": [ /* all current BE sets */, { "name": "new-be", "url": [...] } ] },
  "p2c": { "sets": [ /* all current P2C sets */ ] },
  "virtual_priority": { "sets": [ /* all current VP sets */ ] } }
```

Same rule for **removing** or **editing** a set: start from current, change that one set, submit the whole payload.

`union` is **read-only**. It is how the node and CLI compute effective config. It is **not** applied on write.

---

## 4. Account layers

| Layer | PDA seeds | Who writes | Purpose |
|-------|-----------|------------|---------|
| **Global** | `global-validator-config` | **Manager** | Network-wide defaults + `ConfigLimits` |
| **Validator** | `validator-config` + vote | **Manager** | Live per-vote overlay + `ConfigLimits` (copied from global at `init_validator`) |
| **Proposal** | `validator-proposal` + vote | **Operator** draft → **Manager** approve/reject | Draft config + snapshotted `ConfigLimits` (from validator at propose/update) |

### 4.1. ConfigLimits (size caps)

Versioned enum (`ConfigLimits::V1(ConfigLimitsV1)`), stored on **Global**, **Validator**, and **Proposal**.

| Field | Default | Absolute safety max |
|-------|---------|---------------------|
| `max_url_len` | 256 | 1024 |
| `max_sets_per_section` | 16 | 64 |
| `max_urls_per_set` | 8 | 32 |
| `max_vp_entries_per_set` | 64 | 255 |

- Init order: `global init` → `validator init`.
- Global / validator / proposal writes use that account’s `limits`.
- Manager can change Global/Validator caps via `update_global_limits` / `update_validator_limits` (must be non-zero, ≤ absolute max, and still fit the current payload).

**Effective config** for a vote:

- No validator PDA → global only
- Validator PDA exists → `union_configs(global, validator)` by set name:
  - same `name` → validator **replaces the whole named set** (nested URL lists are not merged entry-by-entry)
  - new `name` → appended
  - a name that exists only on global → kept

Because union only runs on **read**, a validator PDA that omitted a global name still *inherits* that name at runtime. A **proposal** that is meant to keep validator-specific sets must still list **those validator sets plus the new one**, or approve will replace the live validator payload and drop the omitted validator-only names.

---

## 5. Proposal flow

1. Manager `init_validator --operator <OP>` (live = snapshot of **then-current** global)
2. Operator reads **current live** config (`union --vote` and `validator show`)
3. Operator builds **current + new** JSON and `proposal submit` / `update_proposal`
4. Manager `proposal approve` (promote) or `proposal reject` (discard)

Live config is unchanged until approve. After approve, the validator PDA is exactly the proposed payload (full replace).

---

## 6. CLI

[Client Config CLI](../../cli/client_config.md) (`rakurai-client-config`).

Example JSON (full payloads, not deltas):

- [`cli/examples/validator_config.json`](../../cli/examples/validator_config.json) — global-style multi-set
- [`cli/examples/validator_config_overlay.json`](../../cli/examples/validator_config_overlay.json) — extra validator sets **to merge by hand into current**, not to submit alone
