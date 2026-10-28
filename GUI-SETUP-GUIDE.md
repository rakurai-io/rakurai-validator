# How to Enable the GUI

This guide explains how to run the validator GUI server, connect a dashboard client, and manage which client IPs may open WebSocket connections.

The GUI streams real-time validator metrics over WebSocket.

---

## Quick start

Add `--enable-gui` to your validator startup flags:

```bash
agave-validator \
  ...your existing flags... \
  --enable-gui
```

With defaults, the WebSocket server listens on **`127.0.0.1:8765`** and only accepts connections from **`127.0.0.1`**.

Point the dashboard at:

```text
ws://127.0.0.1:8765/websocket
```

On startup you should see a log line similar to:

```text
gui websocket server listening on 127.0.0.1:8765
```

---

## CLI flags

| Flag | Default | Description |
|------|---------|-------------|
| `--enable-gui` | off | Starts the GUI metrics pipeline and WebSocket server. Without this flag, no GUI server is started. |
| `--gui-listen-address HOST:PORT` | `127.0.0.1:8765` | Bind address for the GUI WebSocket server. |
| `--gui-max-websocket-connections COUNT` | `3` | Maximum concurrent GUI WebSocket clients. |

### Example: bind on all interfaces (remote access)

```bash
agave-validator \
  --ledger /mnt/ledger \
  ... \
  --enable-gui \
  --gui-listen-address 0.0.0.0:8765
```

After changing the listen address, point the dashboard at the new WebSocket URL and whitelist client IPs (see [Admin RPC IP whitelisting](#admin-rpc-ip-whitelisting) below).

---

## Connecting the dashboard

The dashboard lives in the [firedancer-frontend](https://github.com/firedancer-io/firedancer-frontend) repository. Clone it and follow the README there to run locally or deploy to production.

Point the dashboard at the validator WebSocket endpoint **`/websocket`** (for example `ws://127.0.0.1:8765/websocket` with default settings).

---

## Admin RPC IP whitelisting

GUI WebSocket connections are gated by an **IP whitelist**. A client is allowed only when its source IP is in the whitelist.

### Default behavior

| Listen address | Initial whitelist |
|----------------|-------------------|
| `127.0.0.1:8765` (default) | `127.0.0.1` is added automatically |
| Any non-loopback address (e.g. `0.0.0.0:8765`, `10.0.1.42:9000`) | **Empty** — all connections rejected until you set the whitelist |

An **empty whitelist rejects every connection**, including after you call `setGuiWhitelist` with an empty array.

Rejected clients receive HTTP **403**; the validator logs:

```text
rejecting gui websocket connection from X.X.X.X: not whitelisted
```

### `setGuiWhitelist` — replace the full whitelist

Each call **replaces** the entire whitelist. Include every client IP that should be allowed.

**Example:**

Run in ledger directory
```bash
echo '{"jsonrpc":"2.0",
    "id":1,
    "method":"setGuiWhitelist",
    "params":[["127.0.0.1","192.168.1.100"]]}' | socat - UNIX-CONNECT:admin.rpc
```
---