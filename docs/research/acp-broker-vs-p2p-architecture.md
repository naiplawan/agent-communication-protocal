# ACP Message Broker vs Direct P2P Architecture

## Summary

ACP is a **pure broker-based** architecture. There is no P2P messaging in this codebase. All agent-to-agent messages pass through a relay that acts as either a **forwarder** (recipient online) or a **broker** (recipient offline). The relay never creates direct connections between agents.

## Research Question

How does ACP route messages between agents — broker mode vs direct P2P — and where does temporary storage fit into the architecture?

---

## Findings

### 1. ACP is a Hub-and-Spoke (Broker) Architecture

From `PROTOCOL.md`:

> ACP is a request/reply protocol for a self-hosted agent mesh. It enables:
> ```
> Human → Agent A → Agent B → Agent C → Human
> ```

All communication flows through agents and relays. There is **no** direct agent-to-agent socket connection in the protocol specification or implementation.

### 2. The Relay Has Two Operational Modes

**`app.rs:131-236` — `send_message` handler:**

```rust
// Try dynamic peer lookup (active peers only)
let peers = state.store.get_peers(false).unwrap_or_default();
let recipient_config = peers.iter().find(|p| p.agent_id == *recipient_agent);

if recipient_config.is_none() {
    // Broker mode: recipient not registered or offline
    state.store.put(msg_id, envelope, &body.payload)?;
    return send_ok(msg_id, "brokered", None);
}

// Forward mode: recipient is registered and reachable
let forward_url = format!("{}/acp/v1/relay/forward", peer.http_endpoint);
// Relay makes HTTP POST to recipient's endpoint directly
```

| Mode | Condition | Behavior | Response |
|------|----------|----------|----------|
| **Broker** | Recipient not in peer registry OR forward fails | Store in SQLite, wait for poll | `{ status: "brokered" }` |
| **Forward** | Recipient registered and reachable | Relay HTTP POSTs to recipient | `{ status: "forwarded", next_hop: "..." }` |

### 3. No P2P Connections Exist in the Codebase

A grep across all source files (`.rs`, `.py`, `.ts`, `.tsx`) for `p2p`, `P2P` returns zero results.

The relay (`app.rs`) makes outbound HTTP connections **from the relay server itself** to reach recipients — it does not facilitate direct agent-to-agent connections.

### 4. Message Storage: SQLite Only

From `main.rs:18-19`:

```rust
let db_path = std::env::var("ACP_DB_PATH")
    .unwrap_or_else(|_| "/tmp/acp-messages.db".to_string());
```

| Env Var | Default | Purpose |
|---------|---------|---------|
| `ACP_DB_PATH` | `/tmp/acp-messages.db` | SQLite database location |

This is a **persistent SQLite database**, not a temporary file. It stores:
- All messages (brokered and forwarded)
- Peer registry
- Message delivery status

The file is NOT created per-message. It is a single database file for all state.

### 5. Forward Failure Triggers Re-Broker

From `app.rs:221-232`:

```rust
Err(e) => {
    let err_str = format!("{}", e);
    if err_str.contains("Connection refused") || err_str.contains("connection reset")
        || err_str.contains("Connection timed out")
    {
        // Stale peer removed, message re-brokered
        state.store.remove_peer(recipient_agent).ok();
        state.store.put(msg_id, envelope, &body.payload).ok();
        return send_ok(msg_id, "brokered", None);
    }
}
```

If a forward fails due to connection issues, the peer is removed from the registry and the message is brokered instead.

### 6. Polling for Offline Delivery

From `app.rs:240-255` — `get_pending`:

```rust
pub async fn get_pending(...) {
    let agent_id = agent_id_from_iss(&claims.iss);
    let messages = state.store.get_all_pending(&agent_id).unwrap_or_default();
    with_cors(Json(serde_json::json!({ "messages": messages })))
}
```

Agents that were offline poll the relay to retrieve their queued (brokered) messages.

### 7. Protocol Supports WebSocket Streams for Replies, Not P2P

From `PROTOCOL.md:176-205` — WebSocket is only for **streaming replies back through the reply path**:

```
STREAM_START (seq=0, total=N) → STREAM_CHUNK (seq=1..N-1) → STREAM_END (seq=N, final=true)
```

The `reply_to.path` field carries an ordered array of `agent_id@machine_id` for return routing. The WebSocket connection is to the **originator's agent**, not a direct P2P channel between two arbitrary agents.

---

## Interpretation

### Why Users Might Perceive "P2P Temp Files"

1. **`/tmp/acp-messages.db`** — The default SQLite path may be perceived as a "temp file." It is actually a persistent database.

2. **`forwarded` response** — When a message is forwarded, the response contains `next_hop` with the recipient's URL, which might look like direct P2P even though the relay made the connection.

3. **Stale peer removal + re-broker** — When an agent goes offline, messages shift from `forwarded` to `brokered` mode, which could feel like switching between "direct" and "relayed" modes.

### Correct Mental Model

```
┌─────────────────────────────────────────────────────────────┐
│                        Relay (Hub)                          │
│                                                             │
│   Agents register here ──► Peer Registry (in-memory + DB)  │
│                                                             │
│   All messages pass through here                            │
│   ├── Online?  → HTTP forward (relay connects to recipient) │
│   └── Offline? → Store in SQLite → poll to retrieve        │
│                                                             │
│   SQLite: /tmp/acp-messages.db (or /data/acp-messages.db)  │
└─────────────────────────────────────────────────────────────┘
          ▲                │                    ▲
          │                │                    │
     registers         forwards             polls for
          │           or brokers           pending msgs
          │                │                    │
   ┌──────┴──┐      ┌──────┴──┐          ┌──────┴──┐
   │ Agent A │      │ Agent B │          │ Agent C │
   └─────────┘      └─────────┘          └─────────┘
```

---

## Conflicting Evidence and Caveats

| Claim | Status |
|-------|--------|
| ACP is purely broker-based | **Confirmed** — no P2P code found |
| Messages are stored in SQLite | **Confirmed** — `Store::put()` writes to SQLite |
| Forward creates direct agent connection | **False** — relay makes the HTTP call |
| Temp files are created per message | **False** — single SQLite database |
| WebSocket enables P2P | **False** — WebSocket is reply path only |

**Unknown**: Whether a separate component (not in this repo) creates P2P temp files for Tailscale, WireGuard, or similar VPN tunnels used for transport.

---

## References

- `acp-server/relay-server/src/app.rs:131-236` — `send_message` with broker/forward logic
- `acp-server/relay-server/src/app.rs:240-255` — `get_pending` polling handler
- `acp-server/relay-server/src/main.rs:18-19` — SQLite default path
- `acp-agent/PROTOCOL.md` — ACP specification v1.0
- `acp-server/relay-server/src/app.rs:221-232` — stale peer removal and re-broker
