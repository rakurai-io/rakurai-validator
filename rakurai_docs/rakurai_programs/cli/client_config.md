# Client Config CLI (`rakurai-client-config`)

Manage on-chain **block-engine** (recv bundles), **post-pack / P2C** (send txns for backrun), and **virtual-priority** (percent of a tip account’s tip) settings for Rakurai validators.

**Audience:** Rakurai ops (manager) and validator operators (proposals).  
**Program:** [`rakurai_client_config`](../programs/rakurai_client_config/README.md).  
**Install:** [CLI overview](./README.md#2-installation).

This CLI does **not** manage TCA / PSA / MCA money accounts — only scheduler **endpoints and virtual-priority tip maps**.

---

## 1. Full payload: current + new

`global update`, `validator update`, `proposal submit`, and `proposal` update **replace the entire on-chain `Config`**. There is no patch/merge on write.

**To add a block-engine, P2C, or virtual-priority set:**

1. Dump current config (`global show`, `validator show`, or `union --vote`)
2. Copy it
3. Append the new named set (keep every set you still want)
4. Submit that file

Do **not** submit a JSON that only contains the new set. That file becomes the whole PDA contents; other sets on that account are deleted.

`union` is a **read** of global ∪ validator. It does not change how writes work.

Worked example — add P2C `p2c-new-1` on global when global already has `p2c-main-1` and `p2c-main-2`:

```json
{
  "block_engine": { "sets": [ /* unchanged: copy from global show */ ] },
  "p2c": {
    "sets": [
      { "name": "p2c-main-1", "url": [{ "url": "https://p2c1.example" }] },
      { "name": "p2c-main-2", "url": [{ "url": "https://p2c2.example" }] },
      { "name": "p2c-new-1", "url": [{ "url": "https://p2c-new.example" }] }
    ]
  },
  "virtual_priority": { "sets": [ /* unchanged: copy from global show */ ] }
}
```

Same pattern for a new **block engine** or **virtual-priority** set. To add a URL *inside* an existing set, copy that set’s current `url` list and append the URL.

[`examples/validator_config_overlay.json`](./examples/validator_config_overlay.json) is **not** a write template. It only shows extra validator-named sets. Fold those extras into a full current snapshot before `validator update` or `proposal submit`.

---

## 2. Layers

| Account | Who | Meaning |
|---------|-----|---------|
| **Global** | Manager | Defaults + size caps for every validator |
| **Validator** (per `--vote`) | Manager | Live overlay + size caps (proposals use these) |
| **Proposal** | Operator → manager approve/reject | Draft; live unchanged until approve |

Effective config the node uses = **global ∪ validator** (by set name). Validator optional. `union` prints that merge.

Size caps (`ConfigLimits::V1`) live on global/validator/proposal PDAs (≤ absolute safety max). Init: `global init` → `validator init`. Proposals snapshot validator limits on submit/update.

---

## 3. JSON shape

Each file has all three sections. See [`examples/validator_config.json`](./examples/validator_config.json).

```json
{
  "block_engine": {
    "sets": [
      {
        "name": "be-main-1",
        "url": [
          {
            "url": "https://be1.example",
            "max_bundles": 100,
            "period_ms": 100,
            "max_bundle_burst": 0
          }
        ]
      }
    ]
  },
  "p2c": {
    "sets": [
      { "name": "p2c-main-1", "url": [{ "url": "https://p2c1.example" }] }
    ]
  },
  "virtual_priority": {
    "sets": [
      {
        "name": "vp-main-1",
        "url": [{ "key": "<PUBKEY>", "value": 0.1 }]
      }
    ]
  }
}
```

| Section | Fields |
|---------|--------|
| Block engine | `url`, `max_bundles`, `period_ms`, `max_bundle_burst` (`0` = unlimited / treat burst as `max_bundles` when quota is set). Scheduler **receives bundles** from these URLs. |
| P2C | `url` only. Scheduler **sends** packed txns here for arbitrage / backrun (point of no return; no front-running). See [Post-pack confirmations](../../transaction_inclusion/post_pack_confirmations.md). |
| Virtual priority | `key` (tip-account pubkey), `value` (fraction of that tip in `[0.0, 1.0]`; e.g. `0.1` = 10%) |

`name` is truncated to 32 bytes. Omit `--config-file` on `global init` to create empty sets, then update with a full file.

---

## 4. Global flags

| Flag | Default | Description |
|------|---------|-------------|
| `-k`, `--keypair` | `~/.config/solana/id.json` | Manager or operator, depending on command |
| `-u`, `--url` | `t` (testnet) | RPC URL or `l` / `t` / `d` / `m` |
| `-p`, `--program-id` | `4uGNMjJFxgE3TfEiPmSpvfwYah12QZbaWWZDJqZvA9F4` | Program ID |

```sh
rakurai-client-config --url <RPC_URL> --program-id <PROGRAM_ID> --keypair <KEYPAIR> <COMMAND>
```

---

## 5. Commands

Always pass a **full** `--config-file` on update / submit.

### Global (manager)

```sh
rakurai-client-config global init --config-file cli/examples/validator_config.json
# Optional caps on init (defaults shown):
#   --max-url-len 256 --max-sets-per-section 16 --max-urls-per-set 8 --max-vp-entries-per-set 64
rakurai-client-config global show
# Edit the printed JSON (current + new), save, then:
rakurai-client-config global update --config-file /tmp/global-current-plus-new.json
rakurai-client-config global set-limits --max-url-len 256 --max-sets-per-section 16
rakurai-client-config global close
```

### Validator live PDA (manager)

```sh
rakurai-client-config validator init --vote <VOTE_PUBKEY> --operator <OPERATOR_PUBKEY>
rakurai-client-config validator set-operator --vote <VOTE_PUBKEY> --operator <OPERATOR_PUBKEY>
rakurai-client-config validator set-limits --vote <VOTE_PUBKEY> --max-url-len 256
rakurai-client-config validator show --vote <VOTE_PUBKEY>
rakurai-client-config validator update --vote <VOTE_PUBKEY> --config-file /tmp/validator-current-plus-new.json
rakurai-client-config validator close --vote <VOTE_PUBKEY>
```

`validator init` copies **then-current global** config **and limits**. Later global edits do not rewrite existing validator PDAs.

### Proposal (operator → manager)

```sh
# Operator: current validator/union JSON + new sets
rakurai-client-config -k operator.json proposal submit --vote <VOTE> \
  --config-file /tmp/proposal-current-plus-new.json
rakurai-client-config proposal show --vote <VOTE>

# Manager
rakurai-client-config -k manager.json proposal approve --vote <VOTE>
rakurai-client-config -k manager.json proposal reject --vote <VOTE>
```

### Union

```sh
rakurai-client-config union
rakurai-client-config union --vote <VOTE_PUBKEY>
```

---

## 6. Proposal flow (step by step)

Goal: operator proposes a **new P2C set** for one vote. The file they submit is the **entire** live validator config after approve — keep every current set and append the new one.

Placeholders: `<VOTE>`, manager keypair `manager.json`, operator keypair `operator.json`.

### Step 1 — Manager: create the live validator PDA and name the operator

```sh
rakurai-client-config -k manager.json validator init \
  --vote <VOTE> \
  --operator <OPERATOR_PUBKEY>
```

This copies **then-current global** onto the validator PDA. Only this operator can `proposal submit` for that vote.

### Step 2 — Operator: dump what is live today

Use whichever snapshot you will edit. `union --vote` is the **effective** config (global ∪ validator). `validator show` is only the validator PDA.

```sh
rakurai-client-config union --vote <VOTE>
# or
rakurai-client-config validator show --vote <VOTE>
```

Copy the printed `block_engine` / `p2c` / `virtual_priority` sets into a file, e.g. `/tmp/proposal-current-plus-new.json`.

### Step 3 — Operator: add the new set (keep current)

Do **not** send a file that only contains the new set. Example — current P2C `p2c-main-1` stays, new `p2c-op-1` is appended:

```json
{
  "block_engine": { "sets": [ /* copy from show / union — unchanged */ ] },
  "p2c": {
    "sets": [
      { "name": "p2c-main-1", "url": [{ "url": "https://p2c1.example" }] },
      { "name": "p2c-op-1", "url": [{ "url": "https://p2c-operator.example" }] }
    ]
  },
  "virtual_priority": { "sets": [ /* copy from show / union — unchanged */ ] }
}
```

Same idea for a new block-engine or virtual-priority set.

### Step 4 — Operator: submit the draft

Live validator config is **unchanged** until the manager acts.

```sh
rakurai-client-config -k operator.json proposal submit \
  --vote <VOTE> \
  --config-file /tmp/proposal-current-plus-new.json

rakurai-client-config proposal show --vote <VOTE>
```

Re-run `proposal submit` with an updated full file to replace the draft before approve.

### Step 5 — Manager: approve or reject

```sh
# Approve: proposal payload becomes the entire live validator config; proposal PDA is closed
rakurai-client-config -k manager.json proposal approve --vote <VOTE>

# Reject: live validator config stays as-is; proposal PDA is closed
rakurai-client-config -k manager.json proposal reject --vote <VOTE>
```

After **approve**, `validator show --vote <VOTE>` should match the JSON from step 3. `union --vote <VOTE>` is that overlay merged with global by set name.

---

## 7. Help

```sh
rakurai-client-config --help
rakurai-client-config global --help
rakurai-client-config validator --help
rakurai-client-config proposal --help
```
