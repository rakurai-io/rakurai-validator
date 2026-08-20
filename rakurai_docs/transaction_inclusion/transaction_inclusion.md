# Rakurai Transaction Inclusion Network (TIN) — Guide

Bundles, virtual prioritization, and post-pack confirmations for landing transactions on Rakurai validators.

**Audience:** Block engines, searchers, and partners integrating transaction landing with Rakurai.

**Related:** [Tips FAQ](./rakurai_tip_manager_faqs.md) · [Post-pack confirmations](./post_pack_confirmations.md)

---

## 1. Bundle support

Rakurai supports multiple block engines — transaction landing services can send bundles directly to Rakurai nodes. To enable this, share an endpoint with the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) that returns a list of block engine addresses run by your service. Rakurai validators automatically connect to the lowest-latency endpoint (just like Jito's).

Example URL: *https://mainnet.block-engine.jito.wtf*

### 1.1. Endpoint: GetBlockEngineEndpoints

Response:

```
global_endpoint {
  block_engine_url: "https://gateway.your-provider.com"
  shredstream_receiver_address: ""
}
regioned_endpoints {
  block_engine_url: "https://fra.gateway.your-provider.com"
  shredstream_receiver_address: ""
}
```

See your gRPC proto definition for the full request/response schema.

### 1.2. Minimum requirements

If you only operate one endpoint, returning just `global_endpoint` is fine. `global_endpoint.block_engine_url` must be a valid, reachable block engine URL.

Rakurai will add this URL to an on-chain PDA so that all Rakurai nodes that have opted in for this feature can connect seamlessly.

Once the block engine is connected, you can send bundles directly. Each bundle must include a tip transfer instruction to one of Rakurai's tip accounts.

**Note:** For the time being, we can honor our partners' tip accounts. See [Tips FAQ Q4](./rakurai_tip_manager_faqs.md#4-can-i-use-my-own-tip-account-instead-of-rakurais-eight-accounts) for custom tip account details.

### 1.3. For validators

Validator operators can see which block engines are connected and, if they choose, block any of them through Admin RPC.

The following Admin RPC commands are available to manage connections to secondary block engine URLs.

#### 1.3.1. getBlockEngineUrls

Returns the list of block engine URLs maintained by the scheduler.

Command:

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"getBlockEngineUrls","params":[]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc | jq
```

Example response:

```json
{
    "active_secondary_entries": [],
    "admin_secondary_entries": [],
    "blocklisted_uuids": [],
    "onchain_secondary_entries": [],
    "primary_url": "https://frankfurt.mainnet.block-engine.jito.wtf"
}
```

#### 1.3.2. setBlockEngineUrlBlocklist

Sets the blocklist to disconnect block engine URLs by UUID. Each call replaces the entire blocklist. Pass an empty array to clear it.

Command: Add `"<BlockEngine1>"` UUID to blocklist

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"setBlockEngineUrlBlocklist","params":[["<BlockEngine1>"]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

Command: Clear blocklist

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"setBlockEngineUrlBlocklist","params":[[]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

**Note:** Use `params:[[]]` (one parameter: an empty UUID array).

---

## 2. Virtual priority boost

Rakurai introduces a mechanism to improve inclusion efficiency through virtual priority. To enable this, add an additional transfer instruction (just like Jito tips are paid) to one of Rakurai's tip accounts in your transaction or bundle. Virtual priority can boost both TPU transactions and bundles.

**Note:** A tip is not a replacement for regular priority fees. If a transaction has very low priority fees and a very high tip, the virtual boost will be less significant, thereby discouraging priority-fee cannibalization. For the time being, we can also honor our partners' tip accounts — see [Tips FAQ Q4](./rakurai_tip_manager_faqs.md#4-can-i-use-my-own-tip-account-instead-of-rakurais-eight-accounts).

---

## 3. Post-pack confirmations

Rakurai introduces post-pack confirmations — on-chain transaction updates over gRPC generated from the point of no return (post-pack) in the Solana pipeline.

For the full protocol, Admin RPC reference, and gRPC details, see [Post-pack confirmations](./post_pack_confirmations.md).

### 3.1. Setup

To enable post-pack confirmations, share your gRPC endpoint with the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) where you want to receive updates.

Sample gRPC endpoint to share:

```
https://sample-server.com:20000
```

Rakurai will add its partners' gRPC endpoints on-chain so you can receive updates from Rakurai nodes that have opted in.

Using post-pack has **two money paths** — **PSA first** (P2C Subscription Account), **then MCA** (share backrun profit). See [Post-pack confirmations](./post_pack_confirmations.md#1-what-are-post-pack-confirmations).

1. **[PSA](../rakurai_programs/programs/reward_distribution/README.md#4-psa--prepaid-fee-to-use-post-pack)** — top up the prepaid subscription ([`rakurai-p2c`](../rakurai_programs/cli/p2c_subscription.md)). If it runs dry, the stream stops.
2. **[MCA](../rakurai_programs/programs/reward_distribution/README.md#5-mca--sharing-post-pack-backrun-profit)** — after each epoch, report and send your backrun share ([`rakurai-revshare`](../rakurai_programs/cli/partner_reward_settlement.md#mca-setup-post-pack)). You must hold the MCA report key.

Once added, the consumer will start receiving transactions as `PacketBatch` (`solana_perf::packet::PacketBatch`). Rakurai uses the same Jito packet gRPC protocol (`packet.proto`, `block_engine.proto`) — `Packet` / `PacketBatch` — used by the Jito relayer.

### 3.2. Bundle requirements

When building a bundle, make sure to include:

1. The original post-pack confirmation packet(s) unchanged (i.e., `Packet`)
2. Any additional transactions, e.g., backrun / arb
3. A transaction or instruction with a tip attached to one of Rakurai's tip accounts

Bundles that use post-pack confirmations receive an additional priority boost.

### 3.3. For validators

Validator operators can see which endpoints are receiving post-pack confirmations and, if they choose, block any of them through Admin RPC.

The following Admin RPC commands are available to manage post-pack confirmation configuration.

#### 3.3.1. getPostPackConfirmationConfig

Returns the list of endpoints receiving post-pack confirmations.

Command:

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"getPostPackConfirmationConfig","params":[]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc | jq
```

Example response:

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

#### 3.3.2. setPostPackConfirmationUuidBlocklist

Sets the blocklist to disconnect post-pack confirmation endpoints by UUID. Each call replaces the entire blocklist. Pass an empty array to clear it.

Command: Add `"<PostPackEndpoint1>"` UUID to blocklist

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"setPostPackConfirmationUuidBlocklist","params":[["<PostPackEndpoint1>"]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

Command: Clear blocklist

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"setPostPackConfirmationUuidBlocklist","params":[[]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

**Note:** Use `params:[[]]` (one parameter: an empty UUID array).

---

## Appendix

### A.1. Adding custom tip accounts

To use custom tip accounts, share your tip account list with the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) along with the percentage of tips you will share with Rakurai. This is used to prioritize bundles and transactions. A sample JSON list is as follows:

```json
{
  "tip_account_1": 0.20,
  "tip_account_2": 0.25,
  "tip_account_3": 0.30
}
```

After the epoch ends, settle the agreed share into the validator TCA with the [Partner Tip and MevShare Revenue Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md) (`transfer --revenue-kind Tip`). See also [Tips FAQ Q4](./rakurai_tip_manager_faqs.md#4-can-i-use-my-own-tip-account-instead-of-rakurais-eight-accounts).

### A.2. Rakurai's tip accounts

Mainnet tip accounts are listed in the [Tips FAQ appendix](./rakurai_tip_manager_faqs.md#appendix-program-and-tip-account-addresses).

```
BjqjPHFmwr19YFmkH8CMNJFbj1wzX9k9ngr4am2nQEdq
9CNKnAqJgLA4pL6KByzhhdY4mKoQP5wcPdhJgnvvi5Ve
5wy4C2VMFhHE4i8PWKNS1K4SV275zjNwhLwfKBwajrro
AgMdA97pk2i2Ry4YQ4iVPNrRiFhcH3x3ARUCiQGt3vJG
4Qf8JFV5vmpADXNouoJriQ9KiniT5DENrz9JM2mKGH9m
AuFAFzbzE9dzMajy4RNdyJZBTskeiuJQqT2wd9xoGSRD
8aLaHz8595MAvgxKoBJEyZmDfqQp8CorezFGYnC7CPjy
H6hyJo6rpBmwHbvVuWCEHExJ2bE4rcn1hTPeiBtypus4
```

Testnet tip accounts are also in the [Tips FAQ appendix](./rakurai_tip_manager_faqs.md#appendix-program-and-tip-account-addresses).
