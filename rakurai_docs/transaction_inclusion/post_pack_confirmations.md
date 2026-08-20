# Rakurai Post-Pack Confirmations — Guide

How the validator streams transaction updates to post-pack confirmation endpoints and how searchers and TIN partners respond with bundles.

**Audience:** Searchers, TIN partners, traders consuming post-pack confirmations, and validator operators configuring endpoints.

**Related:** [Tips FAQ](./rakurai_tip_manager_faqs.md) · [Transaction Inclusion Network (TIN)](./transaction_inclusion.md)

---

## 1. What are post-pack confirmations?

The Rakurai scheduler provides **post-pack confirmations** (also called **P2C** — pack-to-chain). As soon as a transaction is scheduled for execution, it forwards the update to configured **post-pack confirmation endpoints** over gRPC. These updates are generated from the **point of no return**. Consumers only see updates just before the transactions imminently become part of the block, which prevents front-running.

Post-pack confirmation uses the **Jito packet gRPC protocol** ([`packet.proto`](../../jito-protos/protos/packet.proto), [`block_engine.proto`](../../jito-protos/protos/block_engine.proto)) — the same `Packet` / `PacketBatch` / `StartExpiringPacketStream` shape used by the Jito relayer.

**What consumers receive:** transactions as Jito `Packet` format ([`packet.proto`](../../jito-protos/protos/packet.proto)) messages (raw Solana wire bytes), streamed over `StartExpiringPacketStream`. Duplicate transactions are suppressed. One `Packet` is sent per transaction per endpoint.

**What you send back:** a **bundle** that includes:

1. The original post-pack confirmation packet(s) (unchanged)
2. Any additional transactions (e.g., backrun / arbitrage)

The protocol mirrors the Jito relayer packet/bundle flow.

Using post-pack has **two separate bills**. They do not mix. Comparison: [Reward Distribution](../rakurai_programs/programs/reward_distribution/README.md).

| Order | Account | What you pay for | Tool |
|-------|---------|------------------|------|
| **1. First** | **[PSA](#2-psa--p2c-subscription-account)** (P2C Subscription Account) | **Access** to the stream — a prepaid subscription priced from SOL stake | [`rakurai-p2c`](../rakurai_programs/cli/p2c_subscription.md) |
| **2. Then** | **[MCA](#3-mca--share-backrun-profit)** (MevShare Collection Account) | **Sharing backrun / arbitrage profit** you made from those updates | [`rakurai-revshare`](../rakurai_programs/cli/partner_reward_settlement.md) |

Endpoints (where the scheduler **sends** you transactions) live in [Client Config](../rakurai_programs/programs/rakurai_client_config/README.md). PSA holds prepaid SOL. MCA holds shared backrun SOL.

Paying a tip to land a transaction is a different path — [Tips FAQ](./rakurai_tip_manager_faqs.md). A post-pack bundle still includes a tip so it is prioritized.

---

## 2. PSA — P2C Subscription Account

**PSA is required to receive the stream.** Anyone who wants post-pack must **top up a prepaid account**. Without a funded PSA, the scheduler will not keep sending you updates.

This is **not** a tip and **not** a share of backrun profit. It is a prepaid **access fee** so you can use post-pack confirmations from a given validator.

Each epoch Rakurai takes a fee based on that validator’s SOL stake: **commission to Rakurai**, **remainder to the validator**. If the account runs dry, after a short grace the **stream is stopped** until you top up.

If `block_reward_conversion_enabled` is on (default), that validator remainder is later converted into a high-priority block reward — do not count it twice. See [double-counting note](./README.md#3-note-do-not-double-count-tips-and-converted-block-rewards).

### 2.1. Why PSA exists

- Post-pack is a live gRPC stream of transactions at the point of no return. That has a cost, priced from the **validator’s staked SOL**.
- One **PSA per service, per validator**. You pay for the validator whose updates you receive.
- MCA (profit share) only applies **after** you already have stream access. You cannot skip PSA and go straight to MCA.

### 2.2. Flow

1. Contact the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) and share your gRPC endpoint — see [post-pack setup](./transaction_inclusion.md#31-setup).
2. Rakurai opens a **PSA** for your service + each validator and adds your endpoint.
3. **You top up** SOL ([`rakurai-p2c fund`](../rakurai_programs/cli/p2c_subscription.md)).
4. After each epoch the stake-based fee is taken from prepaid.
5. Keep the balance funded. **Suspended** means post-pack is off until you clear the deficit.

Walkthrough: [P2C Subscription CLI](../rakurai_programs/cli/p2c_subscription.md) · [PSA model](../rakurai_programs/programs/reward_distribution/README.md#4-psa--prepaid-fee-to-use-post-pack).

---

## 3. MCA — share backrun profit

After you are on the stream (**PSA paid**), backrun / arbitrage **profit sits in your own wallet**. Rakurai cannot drain it. You **report** the agreed share after the epoch, then **send that SOL** into the MCA. Rakurai takes commission; the remainder goes to the **validator**.

You **must hold the MCA report key** (`record_authority`). Without it you cannot update the books.

Same as TCA/PSA: if **`block_reward_conversion_enabled`** is on, the validator remainder is converted to a block reward. Indexers that also count tips or block rewards should not double-count — [note](./README.md#3-note-do-not-double-count-tips-and-converted-block-rewards).

### 3.1. Who this is for

Searchers and TIN partners that:

- Already fund the **PSA**
- Capture MEV (backruns / arbitrage) from those confirmations
- **Share a percentage** of that profit with the validator

One MCA per service per validator.

### 3.2. Register

1. Same setup as PSA: share your endpoint with the Rakurai team — [post-pack setup](./transaction_inclusion.md#31-setup).
2. Rakurai creates your **MCA** and gives you a revenue name plus the report key. Confirm it with Partner CLI `get-account` (`Record auth`).
3. You receive post-pack updates; after each epoch you share profit through the MCA.

Commands: [Partner settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md#mca-setup-post-pack).

### 3.3. During the epoch

**Nothing** is taken automatically. Profit stays in your accounts until the epoch ends. (Custom tips work differently — see [TCA](../rakurai_programs/programs/reward_distribution/README.md#3-tca--tips-for-landing-transactions) and [Tips FAQ](./rakurai_tip_manager_faqs.md#61-leader-turn-stage-every-leader-turn).)

### 3.4. After the epoch

1. **Report** the share owed once ([Partner CLI `record-revenue`](../rakurai_programs/cli/partner_reward_settlement.md#35-record-revenue-mca-only)) — books only; no SOL moves.
2. **Send** that SOL into the MCA (`transfer --revenue-kind Mev-share`).
3. Rakurai’s commission is taken; the remainder is paid to the validator.

> If you do not report and send within **2 epochs**, post-pack priority stops after a two-epoch grace.

Model: [MCA](../rakurai_programs/programs/reward_distribution/README.md#5-mca--sharing-post-pack-backrun-profit).

---

## 4. How to use P2C

### 4.1. Setup

To enable post-pack confirmations, share your gRPC endpoint with the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) where you want to receive updates.

Sample gRPC endpoint to share:

```
https://sample-server.com:20000
```

Rakurai adds partner endpoints on-chain so you can receive updates from Rakurai nodes that have opted in. Keep the matching **[PSA](#2-psa--p2c-subscription-account)** funded, or the stream stops.

Once added, you receive transactions as `PacketBatch` (`solana_perf::packet::PacketBatch`) over the Jito packet gRPC protocol.

### 4.2. Transaction / packet structure

[`packet.proto`](../../jito-protos/protos/packet.proto)

For each transaction, the validator sends one `PacketBatchUpdate` with `msg = batches`:

```
PacketBatchUpdate
  └── batches: ExpiringPacketBatch
        ├── header.ts
        ├── batch: PacketBatch
        │     └── packets[]: Packet
        │           ├── data    ← raw Solana wire transaction bytes
        │           └── meta    ← Packet meta (size, addr, port, flags, sender_stake)
        └── expiry_ms = 0
```

**Decode in Rust:**

```rust
use solana_transaction::versioned::VersionedTransaction;

let txn: VersionedTransaction = bincode::deserialize(&packet.data)?;
```

`packet.data` is the raw Solana wire transaction. Decode it, inspect accounts and instructions, then decide whether to backrun.

### 4.3. Send a bundle

Receive the post-pack confirmation `Packet`, then send back a **bundle** (`SendBundle` on the searcher / block-engine path — [`searcher.proto`](../../jito-protos/protos/searcher.proto), [`bundle.proto`](../../jito-protos/protos/bundle.proto)).

When building the bundle, include:

1. The original post-pack confirmation packet(s) **unchanged** (the same `Packet` you received)
2. Any additional transactions (e.g., backrun / arbitrage)
3. A transaction or instruction with a **tip** to one of [Rakurai’s tip accounts](./rakurai_tip_manager_faqs.md#appendix-program-and-tip-account-addresses)

Bundles that use post-pack confirmations receive an additional priority boost.

A `Bundle` is a header plus a list of `Packet`s — the same packet shape as the stream:

```
Bundle
  ├── header
  └── packets[]: Packet     ← original post-pack packet(s) first, then your txs
```

---

## 5. Commands

**Searcher / TIN partner (on-chain money paths):**

| What | CLI |
|------|-----|
| Fund and inspect **PSA** | [`rakurai-p2c`](../rakurai_programs/cli/p2c_subscription.md) |
| Report and settle **MCA** | [`rakurai-revshare`](../rakurai_programs/cli/partner_reward_settlement.md#mca-setup-post-pack) |

**Validator operators** use Admin RPC below to inspect which post-pack endpoints are active and to blocklist services. **Registering** a new endpoint (adding your gRPC URL on-chain) is not done here — share your endpoint with the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official); see [TIN — post-pack setup](./transaction_inclusion.md#31-setup).

Admin IPC is request/response: keep the socket open briefly so `socat` can read the reply before stdin closes.

### 5.1. getPostPackConfirmationConfig

Returns the live status maintained by the scheduler (admin + on-chain merge, blocklist, and what is actually connected).

| Field | Description |
|-------|-------------|
| `onchain_entries` | Entries loaded from the on-chain PDA |
| `blocklisted_uuids` | Endpoint UUIDs blocked via `setPostPackConfirmationUuidBlocklist` |
| `blocklisted_entries` | Full merged entries whose `uuid` is blocklisted (url + uuid) |
| `active_entries` | Merged admin + on-chain (admin wins on same URL), excluding blocklisted UUIDs — these are the endpoints receiving scheduler updates |

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"getPostPackConfirmationConfig","params":[]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc | jq
```

**Example response:**

```json
{
  "admin_entries": [
    {"url":"http://127.0.0.1:20000","uuid":"PostPackConfig2"},
    {"url":"http://127.0.0.1:10000","uuid":"PostPackConfig1"}
  ],
  "onchain_entries": [],
  "blocklisted_uuids": ["PostPackConfig1"],
  "blocklisted_entries": [
    {"url":"http://127.0.0.1:10000","uuid":"PostPackConfig1"}
  ],
  "active_entries": [
    {"url":"http://127.0.0.1:20000","uuid":"PostPackConfig2"}
  ]
}
```

### 5.2. setPostPackConfirmationUuidBlocklist

Blocklists post-pack confirmation endpoints by **UUID**. Each call **replaces** the full blocklist. Pass an empty array to clear.

Blocklisted UUIDs are removed from `active_entries` on the next scheduler config sync. If a blocklisted endpoint already has an open gRPC connection, it is torn down immediately on sync; other endpoints stay connected.

**Example — block one endpoint by UUID:**

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"setPostPackConfirmationUuidBlocklist","params":[["PostPackConfig1"]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

**Example — clear blocklist (reconnect blocklisted endpoints on next sync):**

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"setPostPackConfirmationUuidBlocklist","params":[[]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

**Note:** Use `params:[[]]` (one parameter: an empty UUID array). `params:[]` omits the parameter and will not clear the blocklist.
