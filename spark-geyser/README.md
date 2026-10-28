# Rakurai Spark Geyser

## 1. Introduction

Rakurai Spark Geyser is a Solana validator plugin that streams validator events to external systems via *ZeroMQ*. It serves as a lightweight forwarding layer, ingesting updates from the validator through the Geyser plugin interface and publishing structured event data (for example, transactions, accounts, blocks, and slots) to downstream consumers for indexing, processing, and real-time analytics.

The plugin is designed to be **extremely lightweight**, focusing on:

- Low-latency event forwarding
- Minimal processing inside the validator

This ensures the plugin **does not impact validator performance**, including:

- Block time
- Vote latency
- Rewards
- Memory usage

## 2. Geyser contents

This directory includes everything required to run the Geyser plugin:

- **Prebuilt binaries** under [`pre-build/`](./pre-build/) (`jito/` and `rakurai/`)
- **Configuration file** (`config.json` in-repo, plus copies next to each prebuilt)
- **Build scripts** under [`scripts/`](./scripts/)
- **Documentation** covering basic usage, including how to load and unload the plugin in the validator and the overall workflow

> **Note:** Prefer the matching prebuilt under `pre-build/` for your validator line (stock Jito vs Rakurai). Do not load a stock-jito `.so` into a Rakurai validator (ABI mismatch → segfault).

---

## 3. ZeroMQ patterns

- **Supported patterns:**
  - `PubSub` → PUB socket for broadcast
  - `PushPull` → PUSH socket for load-balanced consumers
- **Supported transports:**
  - `tcp://` for network communication
  - `ipc://` for local UNIX socket communication
- **Examples:**
  - `PubSub_tcp://0.0.0.0:4444`
  - `PushPull_ipc:///tmp/account.sock`
- **Roles:**
  - **PubSub:** Geyser acts as **server**. Open TCP ports only to trusted clients (see [§4.5](#45-restricting-who-can-consume)).
  - **PushPull:** Geyser acts as **client**, connecting to an existing socket.

---

## 4. Configuration

The Geyser plugin supports two ZMQ operating modes: **PubSub** and **PushPull**. Both modes define how data is delivered from the Geyser plugin to external consumers, but they follow different communication patterns.

### 4.1. PubSub mode

In PubSub mode, the Geyser plugin behaves as a **server (publisher)** and broadcasts events to multiple subscribers.

- **Geyser role:** PUB (binds and publishes events)
- **Consumer role:** SUB (connects as clients)
- One PUB socket can serve multiple SUB consumers without requiring additional ports.

#### 4.1.1. Example

```json
"Slot": {
  "cluster": {
    "addr": "PubSub_tcp://0.0.0.0:5550",
    "hwm": 500
  }
}
```

#### 4.1.2. Characteristics

- One-to-many broadcast model
- Single publisher → multiple subscribers

---

### 4.2. PushPull mode

In PushPull mode, the Geyser plugin behaves as a **client (PUSH)** and sends data to one or more consumer-side **PULL servers**.

- **Geyser role:** PUSH (connects to consumer endpoints)
- **Consumer role:** PULL (binds and receives data)
- Each consumer requires its own configured endpoint.

#### 4.2.1. Example

```json
"Slot": {
  "mds": {
    "addr": "PushPull_tcp://0.0.0.0:5550",
    "hwm": 500
  },
  "oms": {
    "addr": "PushPull_tcp://0.0.0.0:5551",
    "hwm": 500
  }
}
```

#### 4.2.2. Key characteristics

- Point-to-point
- Geyser actively connects to consumers
- Each consumer requires a dedicated endpoint

---

### 4.3. Plugin configuration

The plugin is configured using a `config.json` file.

This configuration defines:

- Enabled message types
- Consumer routing per message type
- ZMQ socket addresses (PubSub or PushPull)
- High Water Mark (HWM) limits
- Optional logging and version checks

> Each message type can be routed to multiple consumers depending on configuration.

---

### 4.4. Sample configuration

`zmq_auth` is on by default: `tcp://` subscribers must send the built-in credentials unless you set your own. Set your own credentials / allowlist, or turn it off — see [§4.5](#45-restricting-who-can-consume).

```json
{
  "log_level": "info",
  "check_version": true,
  "zmq_auth": {
    "enabled": true,
    "allowed_ips": [],
    "username": null,
    "password": null
  },
  "program_filter_control": {
    "enabled": false,
    "addr": "tcp://0.0.0.0:5600",
    "hwm": 100
  },
  "supported_programs": {
    "orca": [
      "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc"
    ],
    "raydium": [
      "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
      "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK",
      "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
    ],
    "meteora": [
      "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
      "cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG"
    ],
    "drift": [
      "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH"
    ],
    "system": [
      "Sysvar1111111111111111111111111111111111111"
    ]
  },
  "zmq_sockets": {
    "Slot": {
      "cluster": {
        "addr": "PubSub_tcp://0.0.0.0:5550",
        "hwm": 500
      }
    },
    "AccAndTx": {
      "cluster": {
        "addr": "PubSub_tcp://0.0.0.0:5551"
      }
    },
    "Entry": {
      "cluster": {
        "addr": "PubSub_tcp://0.0.0.0:5552",
        "hwm": 500
      }
    },
    "Account": {
      "cluster": {
        "addr": "PubSub_tcp://0.0.0.0:5554"
      }
    },
    "BlockMeta": {
      "cluster": {
        "addr": "PubSub_tcp://0.0.0.0:5555",
        "hwm": 100
      }
    },
    "Transaction": {
      "cluster": {
        "addr": "PubSub_tcp://0.0.0.0:5556"
      }
    }
  },
  "libpath": "./libspark_geyser_plugin-4_4_0-<hash>.so"
}
```

Rakurai validators with `check_version: true` expect the `.so` filename to include the Agave version suffix (`4_4_0` for 4.4.0).

---

### 4.5. Restricting who can consume

By default the stream is neither authenticated nor encrypted. A `PubSub_tcp://0.0.0.0:<port>` socket can be read by **anyone who can reach that port**. ZMQ CURVE is not available, because the bundled libzmq is built without it.

Pick the first option that fits your setup. You can combine them.

| Consumers run…                       | Use                                                      | Code/config change |
| ------------------------------------ | -------------------------------------------------------- | ------------------ |
| On the validator host                | [A. `ipc://` or `127.0.0.1`](#451-a-consumers-on-the-same-host) | Address only |
| On other hosts you control           | [B. Private interface + firewall (+ tunnel)](#452-b-consumers-on-other-hosts) | Address + host firewall |
| Anywhere, as extra defense in depth  | [C. `zmq_auth` allowlist / credentials](#453-c-built-in-allowlist-and-credentials-zmq_auth) | Optional `config.json` block |

#### 4.5.1. A. Consumers on the same host

Nothing leaves the machine.

- **`ipc://` (preferred):** access is controlled by filesystem permissions. Put the sockets in a directory shared by the validator user and a consumer group:

  ```bash
  sudo groupadd -f spark && sudo usermod -aG spark <consumer-user>
  sudo install -d -o <validator-user> -g spark -m 2770 /run/spark   # setgid: sockets inherit group "spark"
  ```

  Run the validator with `UMask=0007` (systemd unit) so the socket file is group-writable. Connecting to a Unix socket requires write permission on it. Then:

  ```json
  "Slot": { "cluster": { "addr": "PubSub_ipc:///run/spark/slot.sock" } }
  ```

  The reader connects with `PubSub_ipc:///run/spark/slot.sock`.

- **`127.0.0.1`:** `PubSub_tcp://127.0.0.1:5550`. Not reachable from the network, but **any local user** can connect. Fine on a single-tenant host.

#### 4.5.2. B. Consumers on other hosts

1. **Bind to the private interface, not `0.0.0.0`:**

   ```json
   "Slot": { "cluster": { "addr": "PubSub_tcp://10.0.0.2:5550" } }
   ```

2. **Allow only consumer IPs on the validator host firewall:**

   ```bash
   # ufw
   sudo ufw allow from 10.0.0.5 to any port 5550:5557 proto tcp
   sudo ufw deny 5550:5557/tcp

   # or iptables
   sudo iptables -A INPUT -p tcp --dport 5550:5557 -s 10.0.0.5 -j ACCEPT
   sudo iptables -A INPUT -p tcp --dport 5550:5557 -j DROP
   ```

3. **Across untrusted networks, add encryption at the network layer.** The ZMQ stream itself is plaintext.
   - **WireGuard (recommended for latency):** bind geyser to the WireGuard address (for example, `PubSub_tcp://10.8.0.1:5550`) and allow the port only on the `wg0` interface.
   - **SSH tunnel (quick setup, adds latency):** keep geyser on `127.0.0.1` and, on the consumer host, run:

     ```bash
     ssh -N -L 5550:127.0.0.1:5550 -L 5551:127.0.0.1:5551 user@validator
     ```

     The reader then connects to `PubSub_tcp://127.0.0.1:5550`.

#### 4.5.3. C. Built-in allowlist and credentials (`zmq_auth`)

This top-level block is optional, but access control is **on by default**: without it, subscribers must send the built-in credentials (`spark_message::DEFAULT_ZMQ_USERNAME` / `DEFAULT_ZMQ_PASSWORD`). It uses libzmq's built-in authentication hook (ZAP, the ZeroMQ Authentication Protocol), which works without CURVE.

```json
{
  "zmq_auth": {
    "enabled": true,
    "allowed_ips": ["10.0.0.5", "10.0.1.0/24", "127.0.0.1"],
    "username": "spark",
    "password": "change-me"
  },
  "zmq_sockets": { "...": "..." }
}
```

| Field         | Meaning |
| ------------- | ------- |
| `enabled`     | Default `true`. `false` turns access control off: any IP may connect without credentials and ZAP is not started. |
| `allowed_ips` | Source IPs or CIDR ranges allowed to connect (IPv4/IPv6). Omitted or empty means any address. Matches the address the validator sees, so this is the address *after* NAT. |
| `username` / `password` | Credentials every subscriber must send (ZMQ `PLAIN`). Set both, or neither to use the built-in defaults. |

Behavior:

- **Secure default:** omitting `zmq_auth`, or leaving `username`/`password` unset, requires the built-in credentials. They ship in the plugin binary and in `spark-message`, so anyone with either can read them: set your own in production. The validator logs a warning at startup while the defaults are in use.
- **Open mode:** `"enabled": false` — any IP may connect without credentials; ZAP is not started.
- Applies to **`PubSub_tcp://`** sockets only. `ipc://` relies on file permissions (option A). For `PushPull`, geyser is the client and only sends to the endpoint you configured, so protect the consumer's PULL endpoint on the consumer side.
- **Fails closed:** an invalid `zmq_auth` block (bad IP/CIDR, only one of username/password) stops the plugin from loading. The stream is never started open by mistake.
- A rejected client never receives data and does not count as a connected client (the stream is only considered connected once a client completes the handshake). Handshake failures on a socket are logged at most once per minute, with a count of the ones not logged and a hint for a credentials/mechanism mismatch. Rejections are also logged at most once per minute per address:
  `ZMQ client rejected | address: 203.0.113.9, reason: address not allowed`.
- **`PLAIN` authenticates but does not encrypt.** The password and the data cross the wire in cleartext. Use it on a private network or inside a tunnel (option B), not as a substitute for one. Keep `config.json` readable only by the validator user (`chmod 600`).

**Client side.** Clients must set PLAIN credentials (the configured ones, or the built-in defaults) **before** `connect`. A client sending credentials to a socket with `"enabled": false`, or none to one that requires them, fails the handshake and receives nothing:

```rust
let sub = ctx.socket(zmq::SUB)?;
sub.set_plain_username(Some("spark"))?;
sub.set_plain_password(Some("change-me"))?;
sub.connect("tcp://10.0.0.2:5550")?;
sub.set_subscribe(b"")?;
```

```python
sub = ctx.socket(zmq.SUB)
sub.plain_username = b"spark"
sub.plain_password = b"change-me"
sub.connect("tcp://10.0.0.2:5550")
sub.setsockopt(zmq.SUBSCRIBE, b"")
```

The example readers send the built-in defaults on `tcp://` unless the environment overrides them (`SPARK_ZMQ_AUTH=off` sends none; see [`example/README.md`](./example/README.md)):

```bash
SPARK_ZMQ_USERNAME=spark SPARK_ZMQ_PASSWORD=change-me \
  cargo run --release --bin reader PubSub_tcp://10.0.0.2:5550
```

A client with wrong or missing credentials, or from an address that is not allowed, connects at the TCP level but gets no messages. The validator log shows the rejection.

---

### 4.6. Live program filter (`program_filter_control`)

Account updates are filtered by owner pubkey using `supported_programs` in config. That list is the **startup default**. An **empty** list sends account updates for **all** owners (every account write on the validator, a very high volume); the validator logs a warning when that happens. Invalid pubkeys in config are skipped with a warning. Optionally, a remote client can replace it at runtime over a ZMQ REP control socket.

```json
"program_filter_control": {
  "enabled": false,
  "addr": "tcp://0.0.0.0:5600",
  "hwm": 100
}
```

| Field     | Meaning |
| --------- | ------- |
| `enabled` | `false` (default) — control socket not started; only config `supported_programs` applies. Set `true` to allow live updates. |
| `addr`    | Bind address (`tcp://` or `ipc://`). Required when `enabled` is true. Plain address — no `PubSub_` prefix. |
| `hwm`     | ZMQ high-water mark for the REP socket. |

Behavior:

- **Both stay:** config seeds the filter at load; client `set` replaces the in-memory set (does not rewrite `config.json`).
- **Merge on set:** non-empty `programs` are added to the current allowlist; response returns the full active list. **Empty list** (`"programs": []`) clears the filter, which sends account updates for **all** owners.
- When `zmq_auth` is active, the control socket uses the same IP allowlist / PLAIN credentials as PUB sockets.
- Ops (JSON over REQ/REP):

  | `op` | Request | Response |
  | ---- | ------- | -------- |
  | `ping` | `{"op":"ping"}` | `{"ok":true}` |
  | `get_supported_programs` | `{"op":"get_supported_programs"}` | `{"ok":true,"count":N,"programs":["…"]}` |
  | `set_supported_programs` | `{"op":"set_supported_programs","programs":["base58…"]}` | `{"ok":true,"count":N,"programs":["…"]}` (full active list; `[]` clears = all owners) |
  | `remove_supported_programs` | `{"op":"remove_supported_programs","programs":["base58…"]}` | `{"ok":true,"count":N,"programs":["…"]}` (remaining active list) |

Example client:

```bash
# list
cargo run -p example --bin control -- tcp://127.0.0.1:5600 get

# merge into allowlist (omit pubkeys to clear)
cargo run -p example --bin control -- tcp://127.0.0.1:5600 set \
  TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA

# with PLAIN (when zmq_auth has username/password)
SPARK_ZMQ_USERNAME=user SPARK_ZMQ_PASSWORD=password \
  cargo run -p example --bin control -- tcp://10.0.0.2:5600 ping
```

---

## 5. Building prebuilts

Use [`scripts/build-prebuilt.sh`](./scripts/build-prebuilt.sh). It rewrites `Cargo.toml`, builds the plugin, and stages `.so` + `config.json` under `pre-build/`.

Mode dirs only stage `libspark_geyser_plugin-*.so` + `config.json`. Rakurai builds fail if any `solana-sdk` patch does not resolve to the expected rev.

**Only build inside this repo** — do not rebuild `jito-solana` / Rakurai validators here.

The plugin code targets **Agave 4.4** (geyser interface 5.0.0). Nothing validator-specific is
hardcoded in the script: for the given branch/tag it reads the validator repo's `Cargo.toml`
(shared crate versions and **all** of its `[patch.crates-io]`), `.gitmodules` and submodule pins
(patches into `solana-sdk/`, `parking_lot/`, ...), `Cargo.lock` (seeds ours, so shared crates
match the validator exactly), `rust-toolchain.toml` (compiler) and version (the `X_Y_Z` in the
`.so` name). Each build ends with `Verified validator patches`.

### 5.1. Jito (`jito-solana`)

Builds against [`jito-foundation/jito-solana`](https://github.com/jito-foundation/jito-solana) at a tag.

```bash
./scripts/build-prebuilt.sh jito v4.4.0-rc.1-jito
./scripts/build-prebuilt.sh jito 4.4.0              # same as tag v4.4.0-jito
```

Output: `pre-build/jito/libspark_geyser_plugin-4_4_0-<hash>.so` + `config.json`.

### 5.2. Rakurai (`rakurai_jito_private` / `rakurai-validator`)

Applies all of the validator's patches, including Rakurai's `solana-sdk` (its geyser interface
adds tick callbacks, its `RewardType` is `#[repr(C)]`): the plugin must see the same types.

```bash
./scripts/build-prebuilt.sh rakurai v4.4.0-rc.1-rakurai.0 --private   # rakurai_jito_private
./scripts/build-prebuilt.sh rakurai v4.4.0-rakurai.0                  # rakurai-validator (once published)
# optional: --sdk-rev <solana-sdk-git-sha>
```

The ref can be a branch or a tag; the script looks it up on the remote (a branch wins if both
exist). `--private` builds from `rakurai-io/rakurai_jito_private` over SSH, using your SSH key
through the git CLI (`CARGO_NET_GIT_FETCH_WITH_CLI`), and stages under `pre-build/rakurai-private/`.
Repo URLs can be overridden with `JITO_GIT`, `RAKURAI_GIT`, `RAKURAI_PRIVATE_GIT` (e.g. a local
mirror: `RAKURAI_PRIVATE_GIT=file:///path/to/rakurai_jito_private`).

Output: `pre-build/rakurai[-private]/libspark_geyser_plugin-4_4_0-<hash>.so` + `config.json`.

Load (both): `agave-validator -l <ledger> plugin load /path/to/<mode>/config.json`, or
`--geyser-plugin-config` at startup; see [pre-build/README.md](./pre-build/README.md).

`scripts/retarget-rakurai.sh` is a thin wrapper around the rakurai mode.

See also [`pre-build/README.md`](./pre-build/README.md).

---

## 6. Message types

The plugin streams several validator event types:

- **Slot** → slot lifecycle updates
- **Account** → account state changes
- **Transaction** → executed transaction information
- **Entry** → ledger entries produced by the validator
- **BlockMeta** → metadata describing a completed block
- **AccAndTx** → combined transaction and related account updates
- **BankCleared** → Alpenglow UpdateParent discarded a bank; drop everything received with that `bank_id`
- **BlockFooter** → Alpenglow block footer (bank hash, block producer time, user agent)

### 6.1. Decoding: Solana v4 and v2 consumers

Messages are defined once in the `spark-message` crate. The same structs build against either Solana generation, so a consumer gets the Solana types it already uses:

| Consumer on | Dependency |
| ----------- | ---------- |
| Current Solana (v3/v4) | `spark-message = { git = "https://github.com/rakurai-io/rakurai-spark" }` |
| Solana 2.x (`solana-sdk` 2.1+) | `spark-message = { git = "https://github.com/rakurai-io/rakurai-spark", default-features = false, features = ["solana-v2"] }` |

Decoding is the same call in both:

```rust
let msg: spark_message::Message = spark_message::deserialize(&bytes)?;
```

With `solana-v2`, fields such as `pubkey`, `signature`, `transaction` and `meta.status` are the consumer's own `solana_sdk` 2.x types (`Pubkey`, `Signature`, `Transaction`, `TransactionError`), so existing v2 code keeps compiling. Enable exactly one of the two features.

The bytes on the wire are identical either way; only two values differ in what a 2.x consumer sees, because 2.x has no exact equivalent:

- `InstructionError::BorshIoError` carries no message on the wire. 2.x gets `BorshIoError("Unknown")`.
- `TransactionError::CommitCancelled` does not exist before Solana 2.2. On 2.1 it decodes as `TransactionError::UnsupportedVersion`.

`spark-message/tests/wire_compat.rs` checks this: the geyser's (v4) encoding is pinned in golden files, and the v2 build decodes the same files.

Both builds decode with wincode at about the same speed; call `spark_message::deserialize` rather than `bincode::deserialize`, which gives the same values but is ~4× slower on transactions and 20–50× on account data. Encode and decode benchmarks: [`bench/README.md`](./bench/README.md).

---

## 7. Event flow

1. The validator emits runtime events.
2. The plugin receives events via Geyser callbacks.
3. Events are converted into internal message objects.
4. Messages are serialized and published to configured ZMQ sockets.

---

## 8. Socket monitor

Each ZMQ socket includes a **socket monitor** (`"monitor_zmq_sockets"`, default `true`) that counts connected clients in real time.

Data is only produced while a client is connected:

- A message type with no connected client on any of its sockets is skipped entirely: the geyser
  callback returns immediately, and nothing is parsed, queued, or serialized.
- When a type has several sockets, only sockets with a connected client receive messages.
- Connections are picked up within about 1 ms. Messages produced before a client connects are not
  replayed.
- `AccAndTx` keeps `Account` and `Transaction` processing running while an `AccAndTx` client is connected.

Set `"monitor_zmq_sockets": false` to always send, whether or not a client is connected.

---

## 9. Quick start

Follow these steps to run Rakurai Spark Geyser with your Solana validator:

1. **Build or pick a prebuilt** under `pre-build/jito/` or `pre-build/rakurai/` (see [§5](#5-building-prebuilts)).
2. **Enable Geyser plugins:** Ensure your validator is running with `--geyser-plugin-always-enabled` if required.
3. **Update configuration:** Edit the staged `config.json` (message types, consumers, ZMQ addresses). Keep `libpath` pointing at the matching versioned `.so`.
4. **Load the plugin:**
   - Start the validator with the Geyser plugin:

     ```bash
     --geyser-plugin-config /path/to/pre-build/rakurai/config.json
     ```

   - Alternatively, load at runtime:

     ```bash
     agave-validator -l /path/to/ledger plugin load /path/to/pre-build/rakurai/config.json
     ```

5. **Unload the plugin:**
   - List loaded plugins:

     ```bash
     agave-validator -l /path/to/ledger plugin list
     ```

   - Unload:

     ```bash
     agave-validator -l /path/to/ledger plugin unload SparkGeyserPlugin
     ```

---

## 10. Benchmarks

Encode and decode speed of the wire format, wincode vs bincode and `solana-v4` vs `solana-v2`: [`bench/README.md`](./bench/README.md).

```bash
cargo run --release -p spark-bench --bin encode
cargo run --release -p spark-bench --bin decode
```
