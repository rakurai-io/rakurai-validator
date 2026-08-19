# Rakurai Post-Pack Confirmations — Guide

How the validator streams transaction updates to post-pack confirmation endpoints and how searchers and transaction inclusion services respond with bundles.

**Audience:** Searchers, transaction inclusion services, traders consuming post-pack confirmations, and validator operators configuring endpoints.

---

## 1. Overview

The Rakurai scheduler provides post-pack confirmations. As soon as a transaction is scheduled for execution, it forwards the update to configured **post-pack confirmation endpoints** over gRPC. These updates are generated from the **point of no return**. Consumers only see updates just before the transactions imminently become part of the block, which prevents front-running.

Post-pack confirmation uses the **Jito packet gRPC protocol** ([`packet.proto`](../../jito-protos/protos/packet.proto), [`block_engine.proto`](../../jito-protos/protos/block_engine.proto)) — the same `Packet` / `PacketBatch` / `StartExpiringPacketStream` shape used by the Jito relayer.

The consumer's job is to receive the post-pack confirmation `Packet`, then send back a **bundle** that includes:

1. The original post-pack confirmation packet(s) (unchanged)
2. Any additional transactions (e.g., backrun / arbitrage)

Duplicate transactions are suppressed. One `Packet` is sent per transaction per endpoint.

**What consumers receive:** transactions as Jito `Packet` format ([`packet.proto`](../../jito-protos/protos/packet.proto)) messages (raw Solana wire bytes), streamed over `StartExpiringPacketStream`.

**What you send back:** a bundle that includes the original post-pack confirmation packet **unchanged**, plus any additional transactions (e.g., arbitrage). The protocol mirrors the Jito relayer packet/bundle flow.

---

## 2. Admin RPC

**Validator operators** use this section to inspect which post-pack endpoints are active and to blocklist services. **Registering** a new endpoint (adding your gRPC URL on-chain) is not done here — share your endpoint with the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official); see [Transaction inclusion — post-pack setup](./transaction_inclusion.md#31-setup).

Admin IPC is request/response: keep the socket open briefly so `socat` can read the reply before stdin closes.

### 2.1. getPostPackConfirmationConfig

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

### 2.2. setPostPackConfirmationUuidBlocklist

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

---

## 3. gRPC protocol

### 3.1. Packet shape

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

---

## 4. Two money paths: PSA then MCA

Using post-pack has **two separate bills**. They do not mix. Comparison: [Reward Distribution](../rakurai_programs/programs/reward_distribution/README.md).

| Order | Account | What you pay for | Tool |
|-------|---------|------------------|------|
| **1. First** | **[PSA](../rakurai_programs/programs/reward_distribution/README.md#4-psa--prepaid-fee-to-use-post-pack)** (P2C Subscription Account) | **Access** to the stream — a prepaid subscription priced from SOL stake | [`rakurai-p2c`](../rakurai_programs/cli/p2c_subscription.md) |
| **2. Then** | **[MCA](../rakurai_programs/programs/reward_distribution/README.md#5-mca--sharing-post-pack-backrun-profit)** (MevShare Collection Account) | **Sharing backrun / arbitrage profit** you made from those updates | [`rakurai-revshare`](../rakurai_programs/cli/partner_reward_settlement.md) |

Endpoints (where the scheduler **sends** you transactions) live in [Client Config](../rakurai_programs/programs/rakurai_client_config/README.md). PSA holds prepaid SOL. MCA holds shared backrun SOL.

---

## 5. PSA — pay to use the stream

Anyone who wants post-pack must **top up a prepaid account**. Each epoch Rakurai takes a fee based on SOL stake: **commission to Rakurai**, **remainder to the validator**. If the account runs dry, after a short grace the **stream is stopped** until you top up.

### 5.1. Flow

1. Contact the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) and share your gRPC endpoint — see [post-pack setup](./transaction_inclusion.md#31-setup).
2. Rakurai opens a **PSA** for your service + each validator and adds your endpoint.
3. **You top up** SOL ([`rakurai-p2c fund`](../rakurai_programs/cli/p2c_subscription.md)).
4. After each epoch the stake-based fee is taken from prepaid.
5. Keep the balance funded. **Suspended** means post-pack is off until you clear the deficit.

Walkthrough: [P2C Subscription CLI](../rakurai_programs/cli/p2c_subscription.md) · [PSA model](../rakurai_programs/programs/reward_distribution/README.md#4-psa--prepaid-fee-to-use-post-pack).

---

## 6. MCA — share backrun profit

After you are on the stream, backrun / arbitrage **profit sits in your own wallet**. Rakurai cannot drain it. You **report** the agreed share after the epoch, then **send that SOL** into the MCA. Rakurai takes commission; the remainder goes to the **validator**.

You **must hold the MCA report key** (`record_authority`). Without it you cannot update the books.

### 6.1. Who this is for

Searchers and transaction-inclusion services that:

- Already pay for the stream (**PSA**)
- Capture MEV (backruns / arbitrage) from those confirmations
- **Share a percentage** of that profit with the validator

One MCA per service per validator.

### 6.2. Register

1. Same setup as PSA: share your endpoint with the Rakurai team — [post-pack setup](./transaction_inclusion.md#31-setup).
2. Rakurai creates your **MCA** and gives you a revenue name plus the report key. Confirm it with Partner CLI `get-account` (`Record auth`).
3. You receive post-pack updates; after each epoch you share profit through the MCA.

Commands: [Partner settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md#mca-setup-post-pack).

### 6.3. During the epoch

**Nothing** is taken automatically. Profit stays in your accounts until the epoch ends. (Custom tips work differently — see [TCA](../rakurai_programs/programs/reward_distribution/README.md#3-tca--tips-for-landing-transactions) and [Tips FAQ](./rakurai_tip_manager_faqs.md#61-leader-turn-stage-every-leader-turn).)

### 6.4. After the epoch

1. **Report** the share owed once ([Partner CLI `record-revenue`](../rakurai_programs/cli/partner_reward_settlement.md#35-record-revenue-mca-only)) — books only; no SOL moves.
2. **Send** that SOL into the MCA (`transfer --revenue-kind Mev-share`).
3. Rakurai’s commission is taken; the remainder is paid to the validator.

> If you do not report and send within **2 epochs**, post-pack priority stops after a two-epoch grace.

Model: [MCA](../rakurai_programs/programs/reward_distribution/README.md#5-mca--sharing-post-pack-backrun-profit).

---

