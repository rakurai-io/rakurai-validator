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

- **Geyser plugin binary** (`libspark_geyser_plugin-<version>.so`)
- **Configuration file** (`config.json`)
- **Documentation** covering basic usage, including how to load and unload the plugin in the validator and the overall workflow

> **Note:** The plugin is provided as a **prebuilt binary** and is intended to be used as a drop-in component for streaming validator events over ZeroMQ.

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
  - **PubSub:** Geyser acts as **server**. Ensure TCP ports are open for clients.
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

```json
{
  "log_level": "info",
  "check_version": true,
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
    "Block": {
      "cluster": {
        "addr": "PubSub_tcp://0.0.0.0:5553",
        "hwm": 100
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
    },
    "Tick": {
      "cluster": {
        "addr": "PubSub_tcp://0.0.0.0:5557"
      }
    }
  },
  "libpath": "./libspark_geyser_plugin-3_1_13.so"
}
```

---

## 5. Message types

The plugin streams several validator event types:

- **Tick** → validator timing events
- **Slot** → slot lifecycle updates
- **Account** → account state changes
- **Transaction** → executed transaction information
- **Entry** → ledger entries produced by the validator
- **BlockMeta** → metadata describing a completed block
- **Block** → full block data constructed from transactions and metadata
- **AccAndTx** → combined transaction and related account updates

---

## 6. Event flow

1. The validator emits runtime events.
2. The plugin receives events via Geyser callbacks.
3. Events are converted into internal message objects.
4. Messages are serialized and published to configured ZMQ sockets.

---

## 7. Socket monitor

Each ZMQ socket optionally includes a **socket monitor** that tracks connection state in real time.

The monitor observes:

- Client connected
- Client disconnected
- Socket closed

This helps avoid sending messages to disconnected sockets and logs connectivity issues.

---

## 8. Quick start

Follow these steps to run Rakurai Spark Geyser with your Solana validator:

1. **Enable Geyser plugins:** Ensure your validator is running with the flag `--geyser-plugin-always-enabled`. If not, restart your validator with this flag.
2. **Update configuration:** Edit the JSON [configuration file](./config.json) with your desired message types, consumers, and ZMQ socket addresses.
3. **Load the plugin:**
   - Start the validator with the Geyser plugin:

     ```bash
     --geyser-plugin-config /path/to/geyser/config.json
     ```

   - Alternatively, you can load the Geyser plugin at runtime:

     ```bash
     agave-validator -l /path/to/ledger plugin load /path/to/geyser/config.json
     ```

4. **Unload the plugin:**
   - To unload a plugin, specify the name of the plugin you want to remove.
   - List all loaded plugins:

     ```bash
     agave-validator -l /path/to/ledger plugin list
     ```

   - Unload the required plugin:

     ```bash
     agave-validator -l /path/to/ledger plugin unload SparkGeyserPlugin
     ```
