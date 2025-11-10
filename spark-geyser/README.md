# Rakurai Spark Geyser

## Introduction

Rakurai Spark Geyser is a Solana validator plugin that streams validator events to external systems using **ZeroMQ**. It acts as a lightweight forwarding layer that receives updates from the validator through the Geyser plugin interface and publishes them to downstream services such as market data systems, order
management engines, or cluster infrastructure.

The plugin is designed to be **extremely lightweight**, focusing on:

- Low-latency event forwarding
- Minimal processing inside validator

This ensures the plugin **does not impact validator performance**, including:

- Block time
- Vote latency
- Rewards
- Memory usage

---

## ZeroMQ Patterns

- Supported patterns:
  - `PubSub` → PUB socket for broadcast
  - `PushPull` → PUSH socket for load-balanced consumers
- Supported transports:
  - `tcp://` for network communication
  - `ipc://` for local UNIX socket communication
- Examples:
  - `PubSub_tcp://0.0.0.0:4444`
  - `PushPull_ipc:///tmp/account.sock`
- **Roles:**
  - **PubSub:** Geyser acts as **server**. Ensure TCP ports are open for clients.
  - **PushPull:** Geyser acts as **client**, connecting to an existing socket.

---

## Configuration

The plugin is configured using a JSON [configuration file](./config.json).  

Configuration defines:

- Which **message types** are enabled
- Which **consumers** receive them
- **ZMQ socket addresses** (PubSub or PushPull)
- Socket **high water mark (HWM) limits**
- Optional socket logging interval  

> **Note:** Each message type can be routed to multiple consumers.

### Sample Config

```json
{
  "log_level": "info",
  "zmq_sockets": {
    "account": {
      "Mds": "PubSub_tcp://0.0.0.0:4444",
      "Cluster": "PushPull_ipc:///tmp/account.sock"
    },
    "transaction": {
      "Oms": "PushPull_ipc:///tmp/tx_queue"
    },
    "slot": {
      "Cluster": "PubSub_tcp://0.0.0.0:5555"
    },
    "block": {
      "Mds": "PushPull_ipc:///tmp/block_queue",
      "Cluster": "PushPull_ipc:///tmp/block_queue"
    }
  },
  "libpath": "./libspark_geyser_plugin-{VERSION}.so"
}
```

---

## Consumers

The plugin supports three categories of downstream consumers:

* **Mds**: Market Data Servers consuming real-time blockchain updates
* **Oms**: Order Management Systems reacting to transaction events
* **Cluster**: Internal infrastructure services (monitoring, indexing, analytics)

---

## Message Types

The plugin streams several validator event types:

* **Tick** → validator timing events
* **Slot** → slot lifecycle updates
* **Account** → account state changes
* **Transaction** → executed transaction information
* **Entry** → ledger entries produced by the validator
* **BlockMeta** → metadata describing a completed block
* **Block** → full block data constructed from transactions and metadata
* **AccAndTx** → combined transaction + related account updates

---

## Event Flow

1. The validator emits runtime events
2. The plugin receives events via Geyser callbacks
3. Events are converted into internal message objects
4. Messages are queued and processed asynchronously
5. Messages are serialized and published to configured ZMQ sockets

---

## Socket Monitor

Each ZMQ socket optionally includes a **socket monitor** that tracks
connection state in real-time.

The monitor observes:

* Client connected
* Client disconnected
* Socket closed

This helps avoid sending messages to disconnected sockets and logs connectivity issues.

---

## Quick Start to Run Geyser

Follow these steps to run Rakurai Spark Geyser with your Solana validator:

1. **Enable Geyser plugins**: Ensure your validator is running with the flag `--geyser-plugin-always-enabled`. If not, restart your validator with this flag.
2. **Update Configuration**: Edit the JSON [configuration file](./config.json) with your desired message types, consumers, and ZMQ socket addresses.
3. **Load the Plugin**:  
    - Start the validator with the Geyser plugin:
    ```bash
      --geyser-plugin-config /path/to/geyser/config.json
    ```
    - Alternative: you can also load geyser plugin at runtime
    ```bash
      agave-validator -l /path/to/ledger plugin load /path/to/geyser/config.json
    ```

1. **Unload the Plugin**: 
     - To unload pulgin you must specifiy the name of plugin you want to unlaod:
     - List all loaded plugins: ```
     ```bash 
       agave-validator -l /path/to/ledger plugin list
     ```
     - Unlaod the required plugin
     ```bash 
       agave-validator -l /path/to/ledger plugin unload SparkGeyserPlugin
     ```


> **Note:** For **PubSub** sockets, Geyser acts as a **server**. Ensure the TCP ports in your configuration are open and accessible.
> For **PushPull** sockets, Geyser acts as a **client**, connecting to an existing socket.

### Runtime Load/Unload
Geyser plugins can be **loaded or unloaded at runtime** without restarting the validator, giving flexibility for maintenance or configuration changes.


## Example: Writer and Reader

### Writer

```bash
# Publish Dummy account updates to a Pub/Sub socket
cargo run --release --bin spark_writer PubSub_tcp://127.0.0.1:4444
```

### Reader

```bash
# Subscribe to any type updates
cargo run --release --bin spark_reader PubSub_tcp://127.0.0.1:4444
```

---