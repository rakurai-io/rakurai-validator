# Setup Guide

This guide covers the requirements for integrating with TIN, including onboarding, gRPC service implementation, and the request/response structures used by Rakurai validators to discover and connect to your block engine and P2C servers.

## 1. Joining TIN

To join the Rakurai Transaction Inclusion Network (TIN), provide the Rakurai team with:

1. **Block Engine and P2C discovery endpoint** — a gRPC endpoint that returns the list of **block engine and P2C server URLs** that Rakurai validators should connect to.

2. **Settlement wallet pubkey** — a wallet public key you control for **PSA** and **MCA** settlements.

Rakurai handles the on-chain registration. Once registered, opted-in Rakurai validators can automatically discover and connect to your block engine endpoints.

**Audience:** Block engines, searchers, and transaction landing services integrating with TIN.

## 2. Sample Application: [`tin_sample_servers`](https://github.com/rakurai-io/tin_sample_servers)
TIN sample servers provides a minimal working reference implementation for integrating with TIN. It demonstrates the gRPC services, authentication flow, discovery mechanism, and basic packet handling.

The repository includes two example servers:
- `p2c_server:` You can use it to connect to Rakurai validators and receive pre-confirmation (P2C) updates.
- `bundles_server:` You can use it to send bundles to Rakurai validators and test bundle submission.

The sample applications are intended as reference implementations for testing and integration. Replace the dummy logic with your own production block engine and P2C implementation.

See [Sample Server](#10-sample-server) for instructions on building and running the sample applications.

> [!NOTE]
> **The connection is outbound from the validator.**
>
> The validator **connects to the URL you provide**. You run a gRPC **server** at that address. Bind to an address that validators can reach. Both `http://` and `https://` URLs are supported.

## 3. Protos

TIN uses these protobuf files. Do not rename any services, RPCs, or fields — validators expect these exact names.

| File | Purpose |
|------|---------|
| [`auth.proto`](../../p2c-protos/protos/auth.proto) | Auth for both bundles and P2C |
| [`block_engine.proto`](../../p2c-protos/protos/block_engine.proto) | Discovery (`GetBlockEngineEndpoints`) and P2C Relayer streams |
| [`packet.proto`](../../p2c-protos/protos/packet.proto) | Packet / batch shapes used on those streams |

For sending bundles (`SubscribePackets`, `SubscribeBundles`), copy the setup from [`tin_sample_servers`](https://github.com/rakurai-io/tin_sample_servers) (`bundles_server`). That sample already includes the full Validator surface you need.

## 4. Contact Rakurai and share your wallet pubkey

On [Discord](https://discord.gg/XS7GmnmCJg) or [Telegram](https://t.me/rakurai_official), share:

1. Your **global / discovery URL**(s) — one for the **block engine (bundles)** path, and one for **P2C** (same URL is fine if one host serves both). Put **all regional endpoints behind** each discovery URL via `GetBlockEngineEndpoints` (`global_endpoint` + `regioned_endpoints`). Only the discovery URL is registered on-chain; regions are not shared with Rakurai one-by-one.
2. A **wallet pubkey you control** (you hold the private key) — used for **PSA / MCA** recording and settlement

Rakurai registers the discovery URL(s) and creates per-validator PSA / MCA (and tip / TCA as needed). Keep that key secure for epoch settlement ([`rakurai-revshare`](../rakurai_programs/cli/partner_reward_settlement.md)). Details: [§5](#5-discovery--global-url-for-bundles-and-p2c).

---

## 5. Discovery — global URL for bundles and P2C

Both bundles and P2C use the same discovery RPC: `GetBlockEngineEndpoints`.

Share **one discovery URL** with Rakurai per path (or one URL if the same host serves both). Validators call that URL, read the list you return, and connect to the closest / lowest-latency endpoint.

| Path | Share with Rakurai | Behind it |
|------|--------------------|-----------|
| Bundles | Discovery URL | Your block-engine hosts |
| P2C | Discovery URL | Your P2C / Relayer hosts |

### 5.1. How it works

1. You implement `GetBlockEngineEndpoints` on your server.
2. You share that server’s public URL with Rakurai (this is the only URL registered on-chain).
3. Validators call it and receive `global_endpoint` + `regioned_endpoints`.
4. They pick the best URL from that list and connect there.

**If you run multiple servers (e.g. FRA, NYC, …):**

- Implement `GetBlockEngineEndpoints` on **every** server.
- Return the **same full list** from each one — every regional URL you want validators to consider.
- You do **not** register each region with Rakurai separately. Add or remove a region by updating the list your servers return.

For a single host, returning only `global_endpoint` is enough.

P2C uses the same RPC. On a P2C-only host, implement `GetBlockEngineEndpoints`.

See [`bundles_server`](https://github.com/rakurai-io/tin_sample_servers/tree/development/bundles_server) and [`p2c_server`](https://github.com/rakurai-io/tin_sample_servers/tree/development/p2c_server).

### 5.2. Request / response

Request:

```protobuf
GetBlockEngineEndpointRequest {}
```

Example response (one global + two regions):

```protobuf
GetBlockEngineEndpointResponse {
  global_endpoint {
    block_engine_url: "https://gateway.your-provider.com"
    shredstream_receiver_address: ""
  }
  regioned_endpoints {
    block_engine_url: "https://fra.gateway.your-provider.com"
    shredstream_receiver_address: ""
  }
  regioned_endpoints {
    block_engine_url: "https://nyc.gateway.your-provider.com"
    shredstream_receiver_address: ""
  }
}
```

| Field | Required? | Meaning |
|-------|-----------|---------|
| `global_endpoint` | Yes | Primary URL for your service. |
| `regioned_endpoints` | Yes if multi-region | Full list of regional URLs |
| `block_engine_url` | Yes | Public URL validators can reach |
| `shredstream_receiver_address` | Optional | Leave `""` unless you use shredstream |

> [!WARNING]
> **Return public URLs only**
>
> Every `block_engine_url` must be reachable from remote validators. Loopback or bind-all addresses (`localhost`, `127.0.0.1`, `0.0.0.0`) will not work.

### 5.3. How to check your server

Before you share a URL with Rakurai, confirm **both** of these respond on that host. If either fails, validators cannot connect.

| Check | RPC | Passes when |
|-------|-----|-------------|
| Discovery | `BlockEngineValidator/GetBlockEngineEndpoints` | You get back `global_endpoint` / `regioned_endpoints` with public URLs |
| Auth | `AuthService/GenerateAuthChallenge` | You get back a `challenge` string |

Install [`grpcurl`](https://github.com/fullstorydev/grpcurl) if needed, then from the proto directory:

**1. Discovery**

```bash
cd path_to_tin_sample_servers/tin_sample_servers/protos/

grpcurl -plaintext -import-path . -proto block_engine.proto \
  <HOST>:<PORT> \
  block_engine.BlockEngineValidator/GetBlockEngineEndpoints
```

**2. Auth** (use `VALIDATOR` for bundles, `RELAYER` for P2C)

```bash
cd p2c-protos/protos
grpcurl -plaintext -import-path . -proto auth.proto \
  -d '{"role":"VALIDATOR","pubkey":"Ed9WjPnZfAXsPttcqxMwj94qsuXVRyBsyXnDkxFva2Zv"}' \
  <HOST>:<PORT> \
  auth.AuthService/GenerateAuthChallenge
```

Run both checks against **every** regional host you list in `regioned_endpoints`, not only the discovery URL.

---

## 6. Auth (shared by both paths)

Both bundles and P2C use `auth.AuthService`. Role differs by path.

| Path | Role |
|------|------|
| **Bundles** (`BlockEngineValidator` streams) | `VALIDATOR` |
| **P2C** (`BlockEngineRelayer` streams) | `RELAYER` |

Flow:

1. `GenerateAuthChallenge` — `{ role, pubkey }` → `{ challenge }`
2. Client signs `"{pubkey}-{challenge}"`, then `GenerateAuthTokens` → `{ access_token, refresh_token }`
3. Later RPCs: `authorization: Bearer <access_token.value>`

```protobuf
GenerateAuthChallengeRequest { role: VALIDATOR, pubkey: <32 bytes> }
GenerateAuthChallengeResponse { challenge: "…" }

GenerateAuthTokensRequest {
  challenge: "<pubkey>-<challenge>"
  client_pubkey: <32 bytes>
  signed_challenge: <64-byte sig>
}
GenerateAuthTokensResponse {
  access_token { value: "…" expires_at_utc { … } }
  refresh_token { value: "…" expires_at_utc { … } }
}
```

Reject identities that are not allowed to connect (e.g. not on the leader schedule / not a Rakurai client).

---

## 7. Set up `BlockEngineValidator` (bundles)

After `VALIDATOR` auth, the validator opens subscribe streams and **you push** packets / bundles for the life of the connection.

| RPC | Required? | Direction |
|-----|-----------|-----------|
| `GetBlockEngineEndpoints` | **Yes** (on discovery host) | Validator → you (unary) |
| `SubscribePackets` | **Yes** for bundles | You → validator (server stream) |
| `SubscribeBundles` | **Yes** for bundles | You → validator (server stream) |
| `GetBlockBuilderFeeInfo` | Optional | Unary fee info |

### 7.1. Request / response schemas

```protobuf
# Request (empty) + Bearer
SubscribePacketsRequest {}
SubscribeBundlesRequest {}

# Stream responses
SubscribePacketsResponse {
  header { ts { … } }
  batch { packets { data: <wire tx> meta { size: … } } }
}

SubscribeBundlesResponse {
  bundles {
    uuid: "…"
    bundle { packets { data: <wire tx> … } }
  }
}
```

Keep streams open for the connection lifetime. Once connected, send bundles with a tip to a Rakurai tip account.

> [!TIP]
> **Recommended tip**
>
> **1,000,000 lamports (0.001 SOL)** per tipped transaction or bundle (in addition to normal priority fees). Tip accounts: [Tips](./tips.md).

---

## 8. Set up `BlockEngineRelayer` (P2C)

After `RELAYER` auth, the validator opens **one** TLS/channel and up to **three** bi-di streams. Scheduler (Mev), Resell, and TPU all send the same `PacketBatchUpdate` message type — decide which path a packet came from by **which gRPC method** accepted it (there is no distinguishing field on the packet itself).

Also expose `GetBlockEngineEndpoints` on `BlockEngineValidator` on the same host (or a dedicated discovery URL) so P2C autoconfig can rank regions — same request/response as [§5.2](#52-request--response).

| RPC | Required? | Default on sample | Content |
|-----|-----------|-------------------|---------|
| `StartExpiringPacketStream` | **Yes** when `resell` | implement for reselling | Leader-time / post-pack (no-boost hash); scheduler only |
| `StartExpiringMevPacketStream` | **Yes** when `mev` | implement for Mev / MCA | Leader-time / post-pack (Mev hash) |
| `StartExpiringTpuPacketStream` | Optional — with `mev` + `enable_tpu_p2c_update` | on (`--disable-tpu-packet-stream` to reject) | Non-leader TPU packets (see [§8.1](#81-differentiating-scheduler-vs-tpu)) |
| `StartP2cUpdateCountStream` | Optional | on (`--disable-p2c-update-count` to reject) | Per-slot counts of transactions successfully sent on the leader-time and TPU streams |

Optional RPCs (TPU, count) may return `UNIMPLEMENTED`. Newer validators keep the leader-time stream(s) and skip the missing optional ones; they do **not** tear down the whole connection. Missing a required stream for an enabled flag (`resell` → PacketStream, `mev` → MevPacketStream) fails the connect.

### 8.1. Differentiating Scheduler vs TPU

The streams are different points in the pipeline. Which leader-time RPC you get depends on the endpoint’s `p2c_type` — and on how you use P2C.

| Stream | When | Who gets it | Meaning |
|--------|------|-------------|----------|
| `StartExpiringPacketStream` | Validator **is** the leader | Endpoints with `resell` | **Post-pack (no boost)** — ReSell hash in `meta.addr`; scheduler only |
| `StartExpiringMevPacketStream` | Validator **is** the leader | Endpoints with `mev` | **Post-pack Mev** — boost-eligible hash in `meta.addr` |
| `StartExpiringTpuPacketStream` | Validator is **not** the leader | Endpoints with `mev` + `enable_tpu_p2c_update` | **TPU path** — validator forwards a received packet to you. This does **not** guarantee inclusion ahead of other transactions. |

**Access rules**

- **Resell / plain:** set `resell` — leader-time on **`StartExpiringPacketStream`** (no tip boost, no TPU). Typically PSA.
- **Backrun / MevShare:** set `mev` (MCA). Opens **`StartExpiringMevPacketStream`**; with `enable_tpu_p2c_update`, also **`StartExpiringTpuPacketStream`**.
- **Both:** set `mev` and `resell` on the same URL — two scheduler connections (distinct hashes); TPU only on the Mev path.
- **Priority boost:** backrun bundles that use **leader-time Mev source transactions** get an additional **20% virtual priority** when landing on Rakurai (Resell stream hashes do not tip-boost).

> [!NOTE]
> **Tell the streams apart by gRPC method, not by a packet field**
>
> All streams send the same `PacketBatchUpdate` shape. There is no `source` field on the packet. If it arrived on `StartExpiringPacketStream`, it is Resell scheduler / post-pack; if on `StartExpiringMevPacketStream`, it is Mev scheduler / post-pack; if on `StartExpiringTpuPacketStream`, it is TPU.

**`meta` differences**

| Field | Scheduler stream | TPU stream |
|-------|------------------|------------|
| `meta.size` | `data.len()` | `data.len()` |
| `meta.addr` | Per-tx string from the scheduler (not an IP) | Validator identity signature over the transaction-signature string (proof this leader emitted the update) |

Treat `meta.addr` as opaque proof / correlation data — do not parse it as a socket address. Full layout: [Using P2C — meta fields](./post_pack/using_p2c.md#22-meta-fields).

### 8.2. Slot on the wire (`expiry_ms`)

Each P2C update (Leader time and TPU) `ExpiringPacketBatch` carries a field named `expiry_ms`:

```protobuf
expiry_ms: <slot as u32>
```

**Despite the name, this is not a millisecond timeout.** On the P2C path it holds the validator’s **slot** (`u32`) at the moment the update was sent. Use it to group, debounce, and time reply bundles against the leader slot that produced the update. When the slot rolls, treat it as a new window (same idea as the count stream below).

### 8.3. `P2cUpdateCount`

When `StartP2cUpdateCountStream` is enabled, the validator pushes one `P2cUpdateCount` **per slot** (and flushes the in-progress slot on disconnect). Each message reports how many transactions were successfully sent on the P2C update streams for that slot:

| Field | Meaning |
|-------|---------|
| `uuid` | Post-pack endpoint UUID |
| `slot` | Slot these counts belong to |
| `scheduler_count` | Transactions successfully sent on the leader-time stream (`StartExpiringPacketStream` or `StartExpiringMevPacketStream`) that slot |
| `tpu_count` | Transactions successfully sent on `StartExpiringTpuPacketStream` that slot |
| `total_count` | `scheduler_count + tpu_count` |
| `p2c_tpu_enabled` | Whether this endpoint has TPU updates enabled in config |

Use this as a health / volume signal without counting packets yourself — confirm the leader is connected, see how much scheduler vs TPU traffic you received that slot, and catch silent drops (heartbeats with no counts).

### 8.4. Packet / count schemas

```protobuf
# Validator → you (scheduler or TPU stream)
PacketBatchUpdate {
  batches {
    batch { packets { data: <wire tx> meta { size: … addr: "<p2c string>" port: 0 } } }
    # Field name is historical; value is the working-bank slot (u32), not a timeout in ms
    expiry_ms: <slot_u32>
  }
}

# Validator → you (count stream)
P2cUpdateCount {
  uuid: "…"
  slot: …
  scheduler_count: …
  tpu_count: …
  total_count: …
  p2c_tpu_enabled: true|false
}

# You → validator (heartbeats on each open Relayer stream)
StartExpiringPacketStreamResponse { heartbeat { count: 1 } }
```

Packet decoding and reply bundles: [Using P2C](./post_pack/using_p2c.md).

---

## 9. Same URL for bundles and P2C

One registered URL must expose:

| Service | What the validator uses |
|---------|-------------------------|
| `auth.AuthService` | `VALIDATOR` **and** `RELAYER` |
| `block_engine.BlockEngineValidator` | `GetBlockEngineEndpoints`, `SubscribePackets` / `SubscribeBundles` |
| `block_engine.BlockEngineRelayer` | `StartExpiringPacketStream` if `resell`; `StartExpiringMevPacketStream` if `mev`; TPU if Mev+flag; count optional |

> [!WARNING]
> **Same URL without the Relayer role**
>
> Bundles can work while P2C fails. Validator log: `SchedulerUpdateNotifier: connection to {url} failed`.

Separate URLs are fine (Validator-only vs Relayer-only hosts). Relayer-only hosts still need `GetBlockEngineEndpoints` for P2C region autoconfig.

---

## 10. Sample server

[tin_sample_servers](https://github.com/rakurai-io/tin_sample_servers) is a working reference: `bundles_server` (Auth + Validator / discovery) and `p2c_server` (Auth + Relayer + `GetBlockEngineEndpoints`, with TPU and count streams on by default).

```bash
cargo build -p bundles_server --release
RUST_LOG=info ./target/release/bundles_server \
  --bind 0.0.0.0:10000 \
  --public-url http://<PUBLIC_HOST>:10000 \
  --allow-any-validator
```

```bash
cargo build -p p2c_server --release
RUST_LOG=info ./target/release/p2c_server \
  --bind 0.0.0.0:10001 \
  --public-url http://<PUBLIC_HOST>:10001
# optional: --disable-tpu-packet-stream --disable-p2c-update-count
```

`--bind` is the listen address (`0.0.0.0` is fine). `--public-url` is what `GetBlockEngineEndpoints` returns — use a host validators can reach (not `127.0.0.1` / `0.0.0.0`). Share that discovery URL with Rakurai. Auth is limited to pubkeys on `getLeaderSchedule` that advertise the Rakurai client id in `getClusterNodes`.

---

## 11. Validator — Check Block Engine and P2C Endpoints

Validators can use **Admin RPC** commands on the validator to inspect the configured and active **Block Engine (bundle)** and **P2C (post-pack)** endpoints, and optionally disconnect/blocklist an endpoint by its UUID.

**Workflow:**

1. Call the **get** method for each service to retrieve the configured and active endpoints.
2. Identify the `uuid` of the endpoint you want to manage.
3. Use the corresponding Admin RPC blocklist command to disconnect and blocklist that endpoint.

Run these commands from the validator host, with `admin.rpc` located in the `/mnt/ledger` directory.

| Service | Inspect | Disconnect |
|---|---|---|
| **Block Engine (Bundles)** | `getBlockEngineUrls` | `setBlockEngineUrlBlocklist` |
| **P2C (Post-Pack)** | `getPostPackConfirmationConfig` | `setPostPackConfirmationUuidBlocklist` |

---

### 11.1. Block Engine — `getBlockEngineUrls`

Shows the primary Block Engine URL plus secondary entries (set on-chain).

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"getBlockEngineUrls","params":[]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc | jq
```

**Example response:**

```json
{
    "primary_url": "https://frankfurt.mainnet.block-engine.jito.wtf",
    "admin_secondary_entries": [],
    "onchain_secondary_entries": [],
    "active_secondary_entries": [],
    "blocklisted_uuids": []
}
```

| Field | Description |
|-------|-------------|
| `primary_url` | CLI-configured Block Engine URL (Jito) |
| `onchain_secondary_entries` | Secondary BE entries loaded from the on-chain config PDA |
| `active_secondary_entries` | Merged on-chain secondary BE (excluding blocklisted UUIDs) |
| `blocklisted_uuids` | Secondary BE UUIDs currently blocklisted |

---

### 11.2. P2C — `getPostPackConfirmationConfig`

Shows post-pack (P2C) endpoints: on-chain and which UUIDs are active.

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"getPostPackConfirmationConfig","params":[]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc | jq
```

| Field | Description |
|-------|-------------|
| `onchain_entries` | Entries from the on-chain PDA (`url`, `uuid`, `enable_tpu_p2c_update`) |
| `blocklisted_uuids` | UUIDs blocked via Admin RPC |
| `active_entries` | Merged on-chain (excluding blocklist) — endpoints that receive Relayer streams |

---

### 11.3. Blocklists

Each `set*` call **replaces** the full blocklist for that service. Pass one UUID array to block; pass an empty inner array to clear blocklist.

**Block Engine** (blocks secondary BE entries by UUID; does not clear `primary_url`):

```bash
# Block one UUID
(echo '{"jsonrpc":"2.0","id":1,"method":"setBlockEngineUrlBlocklist","params":[["<BlockEngine1>"]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc

# Clear — must be params:[[]] not params:[]
(echo '{"jsonrpc":"2.0","id":1,"method":"setBlockEngineUrlBlocklist","params":[[]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

**P2C** (removes matching endpoints from `active_entries` on the next config sync; open connections to blocklisted UUIDs are torn down):

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"setPostPackConfirmationUuidBlocklist","params":[["PostPackConfig1"]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc

(echo '{"jsonrpc":"2.0","id":1,"method":"setPostPackConfirmationUuidBlocklist","params":[[]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

> [!WARNING]
> **Clearing the blocklist**
>
> Use `params:[[]]`. Writing `params:[]` omits the parameter and the blocklist is **not** cleared.

---

## Related

- [tin_sample_servers](https://github.com/rakurai-io/tin_sample_servers) — sample Auth + Validator / Relayer servers
- [Tips](./tips.md) — tip accounts, virtual priority, TCA
- [Post-pack confirmations](./post_pack/README.md) — PSA / MCA
- [Using P2C](./post_pack/using_p2c.md) — packet layout, reply bundles
