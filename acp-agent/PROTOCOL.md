# ACP — Agent Communication Protocol Specification

> **Version 1.0** — Source of truth for the ACP implementation.

---

## Overview

ACP is a request/reply protocol for a self-hosted agent mesh. It enables:

```
Human → Agent A → Agent B → Agent C → Human
              ↑__________________|
              (reply path)
```

Each agent forwards work to the next with a reply path that routes responses back through the chain — no agent needs to know the full topology, only its immediate peers.

---

## Message Envelope

Every ACP message is a JSON envelope wrapping a payload:

```json
{
  "envelope": {
    "msg_id": "msg_a1b2c3d4",
    "corr_id": "msg_a1b2c3d4",
    "origin": {
      "agent_id": "agent-alpha",
      "machine_id": "laptop-1",
      "human_id": "alice@example.com"
    },
    "sender": {
      "agent_id": "agent-beta",
      "machine_id": "server-1"
    },
    "recipient": {
      "agent_id": "agent-gamma",
      "machine_id": "server-2"
    },
    "reply_to": {
      "path": ["agent-alpha@laptop-1"],
      "ws_endpoint": "wss://laptop-1.example.com/acp/stream/alice@example.com"
    },
    "hops": {
      "count": 0,
      "max": 10,
      "trace": []
    },
    "intent": "delegate",
    "content_type": "application/json",
    "priority": "normal",
    "deadline": null
  },
  "payload": { }
}
```

### Field Reference

| Field | Required | Description |
|-------|----------|-------------|
| `msg_id` | Yes | Globally unique message ID (ULID, prefixed `msg_`) |
| `corr_id` | Yes | Correlation ID — the `msg_id` of the originating request |
| `origin` | Yes | Who initiated the chain |
| `sender` | Yes | Current forwarder of this message |
| `recipient` | Yes | Next-hop recipient |
| `reply_to.path` | Yes | Ordered array of `agent_id@machine_id` for return routing |
| `reply_to.ws_endpoint` | No | WebSocket endpoint for streaming reply to origin |
| `hops.count` | Yes | Current hop count (incremented each forward) |
| `hops.max` | Yes | TTL ceiling; message dropped if exceeded |
| `hops.trace` | No | Audit trail of `{agent_id, machine_id, timestamp}` |
| `intent` | Yes | `delegate` \| `reply` \| `ack` \| `error` \| `stream_start` \| `stream_chunk` \| `stream_end` |
| `content_type` | Yes | MIME type of payload |
| `priority` | No | `low` \| `normal` \| `high` (default: `normal`) |
| `deadline` | No | ISO-8601 absolute deadline |

---

## HTTP/REST Endpoints

### `POST /acp/v1/messages/send`

Send a message to a peer agent.

**Request:**
```json
{
  "envelope": { ... },
  "payload": { "task": "review-pr", "pr_url": "..." }
}
```

**Response (202 Accepted):**
```json
{
  "msg_id": "msg_a1b2c3d4",
  "status": "accepted",
  "next_hop": "agent-beta@server-1"
}
```

---

### `GET /acp/v1/messages/{msg_id}/status`

Check delivery status.

**Response (200):**
```json
{
  "msg_id": "msg_a1b2c3d4",
  "status": "pending|delivered|acknowledged|completed|error",
  "delivered_at": "2026-08-06T15:30:02.000Z"
}
```

---

### `POST /acp/v1/messages/{msg_id}/ack`

Acknowledge receipt.

**Request:**
```json
{
  "ack_id": "ack_xyz789",
  "received": true,
  "processed": false,
  "stream_available": true
}
```

---

### `POST /acp/v1/messages/{msg_id}/error`

Report a processing error.

**Request:**
```json
{
  "error_code": "TIMEOUT|UNREACHABLE|INVALID_PAYLOAD|SECURITY_VIOLATION",
  "error_message": "Human-readable",
  "retryable": true
}
```

---

### `POST /acp/v1/stream/init`

Initiate a WebSocket stream.

**Request:**
```json
{
  "msg_id": "msg_a1b2c3d4",
  "corr_id": "msg_a1b2c3d4",
  "stream_type": "reply"
}
```

**Response (200):**
```json
{
  "stream_id": "str_abc123",
  "ws_url": "wss://server-1.example.com/acp/stream/str_abc123"
}
```

---

## WebSocket Protocol

### Connection

```
wss://{host}/acp/stream/{stream_id}?token={stream_token}
```

### Frame Format

```json
{
  "frame": {
    "stream_id": "str_abc123",
    "msg_id": "msg_a1b2c3d4",
    "corr_id": "msg_a1b2c3d4",
    "seq": 0,
    "total": 5,
    "final": false,
    "timestamp": "2026-08-06T15:30:00.000Z"
  },
  "data": { }
}
```

### Streaming Sequence

```
STREAM_START (seq=0, total=N) → STREAM_CHUNK (seq=1..N-1) → STREAM_END (seq=N, final=true)
```

---

## At-Least-Once with Acknowledgments

| Ack Type | Trigger | Purpose |
|----------|---------|---------|
| `hop_ack` | Received at next hop | Delivery confirmed |
| `process_ack` | Final recipient done | Processing complete |
| `stream_ack` | Stream opened | Reply path ready |
| `delivery_ack` | Reply reaches origin | End-to-end confirmed |

**Retry:** 3 attempts max, exponential backoff (1s → 2s → 4s, cap 30s).

**Idempotency:** Messages are idempotent by `msg_id`. Re-delivery returns cached result.

---

## Agent State Machine

```
IDLE → RECEIVED → FORWARDING → PROCESSING → REPLYING → COMPLETE
                  ↓
               FAILED
```

---

## Security

### Signed Tokens

HMAC-SHA256 signed, not JWT:

```
Authorization: ACP-Token <base64(header)>.<base64(payload)>.<base64(sig)>
```

Header: `{"alg":"HS256","typ":"ACP"}`
Payload: `{"iss","aud","exp","iat","msg_id","nonce"}`

### mTLS

For high-security environments. Configure per-peer in `acp-peers.yaml`:

```yaml
auth:
  type: "mtls"
  cert_path: "/etc/acp/certs/peer.crt"
  key_path: "/etc/acp/certs/peer.key"
  verify_path: "/etc/acp/certs/ca.crt"
```

### Per-Hop Auth

Each agent independently verifies incoming auth and signs outgoing.

---

## Configuration (`acp-peers.yaml`)

```yaml
config_version: 1
this_agent:
  agent_id: "agent-beta"
  machine_id: "server-1"
  http_endpoint: "https://server-1.example.com/acp/v1"
  ws_endpoint: "wss://server-1.example.com/acp/stream"
  capabilities: []

peers:
  - agent_id: "agent-alpha"
    machine_id: "laptop-1"
    http_endpoint: "https://laptop-1.example.com/acp/v1"
    ws_endpoint: "wss://laptop-1.example.com/acp/stream"
    auth:
      type: "signed-token"
      secret_path: "/etc/acp/secrets/alpha-signing.key"

security:
  default_auth_type: "signed-token"
  token_ttl_seconds: 3600
  require_https: true

retry:
  max_attempts: 3
  initial_backoff_ms: 1000
  max_backoff_ms: 30000
  backoff_multiplier: 2.0

timeouts:
  hop_ack_ms: 5000
  process_ack_ms: 300000
  stream_init_ms: 10000
```

---

## Message Flow: Multi-Hop Delegation

```
Human → Agent A → Agent B → Agent C → reply → Human
```

**Step 1:** Human POSTs to Agent A. `reply_to.path = [agent-alpha@laptop-1]`

**Step 2:** Agent A forwards to B. `reply_to.path = [agent-alpha@laptop-1]`, `hops.count=1`

**Step 3:** Agent B forwards to C. `reply_to.path = [agent-alpha@laptop-1, agent-beta@server-1]`, `hops.count=2`

**Step 4:** Agent C processes, streams reply via WebSocket, sends `process_ack` back through the path.

**Step 5:** Reply flows: C → B → A → Human's WebSocket endpoint.
