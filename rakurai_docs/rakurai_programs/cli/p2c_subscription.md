# P2C Subscription CLI (`rakurai-p2c`)

Inspect, fund, record, and claim Pack-to-Chain (**P2C**) prepaid subscription escrows used with **post-pack confirmations**.

**Audience:** P2C User/Consumer (searchers / traders / services that consume Rakurai post-pack confirmations).  
**Program:** Reward Distribution **PSA** (`P2CSubscriptionAccount`).  
**Install:** see [CLI overview](./README.md#2-installation). How it works: [PSA](../programs/reward_distribution/README.md#4-psa--prepaid-fee-to-use-post-pack).

---

## 1. What is P2C / post-pack?

**Post-pack confirmations** are how the Rakurai scheduler streams transaction updates to external consumers over **gRPC** (Jito packet protocol). Updates are sent from the *point of no return*—just before transactions become part of the block—so consumers can react (e.g. backrun / arbitrage bundles) without front-running earlier in the pipeline.

From the official guide ([Post-pack confirmations](../../transaction_inclusion/post_pack_confirmations.md)):

- The validator forwards scheduled transactions to configured post-pack endpoints.
- Consumers receive `Packet` / `PacketBatch` messages (`StartExpiringPacketStream`).
- Consumers send back a **bundle** that includes the original post-pack packet(s) unchanged, plus any additional txs.
- Endpoint registration is coordinated with the Rakurai team (on-chain config + Admin RPC for operators).

**This CLI (`rakurai-p2c`)** manages the on-chain **PSA** (P2C Subscription Account) — prepaid subscription escrow for that Pack-to-Chain / post-pack confirmation **user** (per service name + validator vote).

This is **not** MevShare revenue sharing. Sharing post-pack **backrun / MevShare revenue** uses an **MCA** and [`rakurai-revshare`](./partner_reward_settlement.md). Four revenue models: [Reward Distribution](../programs/reward_distribution/README.md).

---

## 2. How the subscription works

Every user who wants **P2C / post-pack confirmations** must **top up** a prepaid escrow and pay a **subscription fee priced from SOL stake**. Each epoch Rakurai deducts that fee: **commission to Rakurai**, remainder to the **validator identity**. If the escrow is not kept funded, P2C access is stopped (after a short grace period).

### 2.1 Prepaid fund (User/Consumer)

1. Rakurai (manager) creates a P2C subscription account for your service name + validator vote.
2. **You fund** the escrow with SOL (`fund`, or a plain `solana transfer` to the PDA).
3. Keep a healthy prepaid balance so epoch claims can succeed without going into deficit.

Any wallet can top up the escrow; the free balance (lamports above rent) is what claims draw from.

### 2.2 Epoch stake → amount due → deduct

At (or after) epoch end:

1. **Manager records** that epoch once: uploads a **stake snapshot** and the computed **`amount_due`** (subscription fee for that epoch).
2. Stake is an **off-chain verifiable** figure — it can be checked against any public source (validator vote stake via RPC, explorers, stake accounts, etc.). The on-chain row stores the snapshot used for pricing so anyone can audit it later.
3. **Manager claims** the epoch: the program deducts `min(remaining due, free prepaid)` from the escrow and pays commission + validator identity.
4. If the escrow has enough SOL, the epoch is fully paid and marked claimed.
5. If underfunded, you can top up and claim again (partial payments accumulate), or the manager can **`force-claim`** to close the epoch and book the shortfall as **deficit**.

### 2.3 Unpaid → grace → P2C stopped

Subscription status is stored on the account (`Active` → `InGrace` → `Suspended`):

| Status | Meaning |
|--------|---------|
| `Active` | Fees current; P2C eligible |
| `InGrace` | One or more underfunded forced closes within the grace window (default **2** epochs) |
| `Suspended` | Unpaid streak exceeded grace — **P2C access is stopped** until the deficit is cleared |

Clearing the open deficit (`clear-deficit`) restores `Active` when the deficit reaches zero. Ops / the scheduler use this status to gate post-pack for the user.

### 2.4 Close anytime → remaining funds returned

You can stop using P2C and **close** the subscription when you no longer need it:

- All recorded epochs must be **claimed** first (no open ledger rows).
- Closing returns **remaining prepaid SOL + rent** to the account **initializer** (who paid rent at create).
- On-chain close is performed by the **manager**; coordinate with Rakurai if you want the escrow closed and residual returned.

---

## 3. Lifecycle (summary)

```text
  create (manager)
       │
       ▼
  fund prepaid (user)  ◄── top up anytime
       │
       ▼
  epoch ends → record(stake, amount_due)  [stake verifiable off-chain]
       │
       ▼
  claim → deduct from prepaid
       │
       ├─ fully paid     → Active
       ├─ underfunded    → top up / claim again, or force-claim → deficit + InGrace/Suspended
       └─ clear-deficit  → back to Active when deficit = 0

  close (manager, after all epochs claimed) → residual SOL + rent → initializer
```

---

## 4. CLI overview

```sh
rakurai-p2c \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  --keypair <KEYPAIR> \
  <COMMAND>
```

Global flags match other Rakurai CLIs: `-k` / `--keypair`, `-u` / `--url`, `-p` / `--program-id`.

| Command | Who | Purpose |
|---------|-----|---------|
| `get-account` | anyone | Show one escrow (balance, status, deficit, ledger) |
| `get-all-accounts` | anyone | List escrows for a service name |
| `fund` | any funder (User/Consumer) | Top up prepaid SOL |
| `record` | **manager** | Upload epoch stake + `amount-due` |
| `claim` | **manager** | Deduct fee from prepaid; optional `--force-claim` |
| `clear-deficit` | any funder | Fund + clear open deficit (restores Active when fully cleared) |

Account **create** / **config** / **close** are manager program instructions (not yet exposed as `rakurai-p2c` subcommands). Use the program SDK / scripts, or ask Rakurai ops.

---

## 5. Examples

```sh
# Inspect balance, status, deficit, and epoch ledger
rakurai-p2c -u t -p <RD_PID> get-account --name mysvc -v <VOTE>

# Fund prepaid (User/Consumer) — keep this funded so claims succeed
rakurai-p2c -u t -p <RD_PID> -k <FUNDER> fund --name mysvc -v <VOTE> -x 1000000000

# After epoch: manager uploads verifiable stake + calculated fee
rakurai-p2c -u t -p <RD_PID> -k <MANAGER> record \
  --name mysvc -v <VOTE> --epoch 750 --stake 1000000 --amount-due 50000000

# Deduct from prepaid (partial OK if underfunded)
rakurai-p2c -u t -p <RD_PID> -k <MANAGER> claim \
  --name mysvc -v <VOTE> --epoch 750 \
  --validator-identity <IDENTITY>

# Force-close underfunded epoch → books deficit (can move toward Suspended / P2C stopped)
rakurai-p2c -u t -p <RD_PID> -k <MANAGER> claim \
  --name mysvc -v <VOTE> --epoch 750 \
  --validator-identity <IDENTITY> \
  --force-claim

# Pay down deficit to restore Active
rakurai-p2c -u t -p <RD_PID> -k <FUNDER> clear-deficit \
  --name mysvc -v <VOTE> \
  --validator-identity <IDENTITY>
```

Anyone may also fund the PDA with a plain `solana transfer` into the escrow address.
