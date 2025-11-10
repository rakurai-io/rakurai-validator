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

## 4. MEV revenue sharing

Post-pack and MEV-share revenue is deposited directly into the **searcher or transaction inclusion service's own account**, which Rakurai does **not** control. When you start using **post-pack**, Rakurai creates a per-validator, per-service **[MevShare Collection Account (MCA)](../rakurai_programs/programs/reward_distribution/README.md#5-tip-and-mevshare-collection-accounts)** for you. That MCA is where you **record** revenue and **transfer** SOL; you **must hold the MCA `record_authority`**.

MCA and TCA both use the same on-chain **[RevenueShareAccount / RevenueShareAccountV1 structure](../rakurai_programs/programs/reward_distribution/README.md#56-revenueshareaccount-revenueshareaccountv1-structure)**; only `share_kind` differs (`MEV_SHARE` vs `TIP`). See the [Reward Distribution program](../rakurai_programs/programs/reward_distribution/README.md) for full account layout, PDA seeds, and ledger fields. Partners record and settle MCA balances with the [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md#mca-setup-post-pack).

### 4.1. Who participates

This flow applies to **searchers** and **transaction inclusion services** that:

- Consume **post-pack confirmations** from Rakurai validators
- Capture MEV (e.g., backruns / arbitrage) from those confirmations
- **Share a percentage of MEV revenue** with the validator

Each participant registers separately with Rakurai and receives one MCA per service per validator.

### 4.2. Register an MCA and endpoint

When you start using **post-pack**, Rakurai creates an **[MCA](../rakurai_programs/programs/reward_distribution/README.md#5-tip-and-mevshare-collection-accounts)** for your service (one per service per validator). That MCA is where you **record** MevShare revenue and **transfer** the corresponding SOL after each epoch.

1. Contact the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) and **share your gRPC endpoint** — see [post-pack setup](./transaction_inclusion.md#31-setup).
2. Rakurai adds your endpoint on-chain and creates your MCA. You receive a **revenue name** (PDA seed) and must hold the MCA **`record_authority`** keypair — only that authority can call `record_revenue` / `record-revenue` on the MCA. Confirm it with the Partner CLI `get-account` (`Record auth` field).
3. After registration, you start receiving **post-pack confirmations** and must **share a percentage of MEV revenue** through your MCA (record, then settle).

See the [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md#mca-setup-post-pack) for MCA setup and commands.

### 4.3. During the epoch

Unlike [TCA (custom tips)](../rakurai_programs/programs/reward_distribution/README.md#51-why-a-tips-collection-account-tca), **nothing is recorded on-chain in the MCA during leader turns**. MEV-share revenue stays in the searcher or transaction inclusion service's own accounts until the epoch ends.

For the parallel custom-tip flow, see [Tips FAQ — leader-turn stage](./rakurai_tip_manager_faqs.md#61-leader-turn-stage-every-leader-turn).

### 4.4. Post-epoch stage (record and settle)

After the epoch ends, the searcher or transaction inclusion service (holding the MCA **`record_authority`**):

1. **Record** — report the revenue share owed for the previous epoch by calling `record_revenue` on the MCA **once** (use Partner CLI [`record-revenue`](../rakurai_programs/cli/partner_reward_settlement.md#34-record-revenue-mca-only) with the `record_authority` keypair). This updates the [RevenueShareAccount ledger](../rakurai_programs/programs/reward_distribution/README.md#56-revenueshareaccount-revenueshareaccountv1-structure) only; no lamports move.
2. **Settle** — transfer the recorded amount into the MCA as SOL using the [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md) (`transfer --revenue-kind Mev-share`), which calls `settle_revenue` for V1 vaults.
3. **Claim** — the reward distribution program splits the settled amount between Rakurai (commission) and the validator (remainder).

> If a service does not record and settle within **2 epochs**, post-pack access and MCA prioritization stop after a two-epoch grace period.

### 4.5. Revenue distribution

Once the recorded amount is settled into the MCA, revenue is split the same way as TCA — see [How Tip and MevShare are distributed](../rakurai_programs/programs/reward_distribution/README.md#53-how-tip-and-mevshare-are-distributed):

- **Client (Rakurai):** the client commission is credited to its account (percentage recorded in the MCA).
- **Validator:** the remaining share is credited to its identity account (with the option to convert it into block rewards when enabled on the MCA).

---
