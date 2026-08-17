# ACP Agent — Agent Communication Protocol

A Rust implementation of the Agent Communication Protocol (ACP) for multi-agent message routing with context handoff support.

## Architecture

```
acp-agent/
├── acp-core/       # Core protocol library
│   └── src/
│       ├── protocol.rs    # Envelopes, IDs, routing
│       ├── security.rs     # HMAC-SHA256 tokens
│       ├── transport.rs   # HTTP/WebSocket clients
│       ├── config.rs      # YAML config loading
│       └── chp.rs         # Context Handoff Protocol
├── acp-agent/      # Rust agent binary + CLI
│   └── src/
│       ├── main.rs        # CLI entry point
│       ├── agent.rs       # ACPAgent struct
│       ├── server.rs      # HTTP server
│       └── signaling.rs   # Relay client
└── acp-dashboard/  # Web dashboard (Vite + React + TypeScript)
    ├── src/
    │   ├── App.tsx        # Main dashboard component
    │   ├── api.ts         # Dashboard API client
    │   ├── types.ts       # TypeScript types
    │   └── components/    # UI components
    └── vite.config.ts     # Vite bundler config
```

## Features

- **Multi-hop delegation** with reply path routing
- **ACP-CHP** (Context Handoff Protocol) for rich task context transfer
- **HMAC-SHA256 signed tokens** for peer authentication
- **Exponential backoff retry** for reliable message delivery
- **WebSocket streaming** for large replies
- **Relay support** for NAT traversal
- **Web dashboard** for monitoring agent activity and inbox

## Build

### Rust Agent
```bash
cargo build --release
```

### Dashboard
```bash
cd acp-dashboard
npm install
# VITE_RELAY_URL is baked in at build time; point it at an agent or the relay
VITE_RELAY_URL=http://localhost:8444 npm run build
```

## Run

```bash
# With config file (default locations: ./acp-peers.yaml, ~/.acp/acp-peers.yaml)
cargo run --release -- run --port 8443

# With signaling (connects to cloud relay)
ACP_RELAY_URL=http://relay:8443 \
ACP_AGENT_ID=my-agent \
ACP_MACHINE_ID=laptop-1 \
ACP_SHARED_SECRET=<secret> \
cargo run --release -- run --port 8443 --use-signaling
```

## CLI Commands

```bash
# Start the agent server
acp-agent run [--port <port>] [--use-signaling]

# Send a message to a peer
acp-agent send <target-agent-id> '<json-payload>'

# Long-poll for incoming messages
acp-agent listen [--poll-interval <seconds>]

# Diagnose connectivity
acp-agent doctor [--target <agent-id>]
```

## HTTP API

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/health` | Status plus `this_agent_id` / `this_machine_id` |
| `GET` | `/acp/v1/capabilities` | Public protocol capabilities |
| `POST` | `/acp/v1/initialize` | Authenticated version and feature negotiation |
| `GET` | `/acp/v1/peers` | Peers from the loaded config |
| `GET` | `/acp/v1/messages/pending` | Inbox and pending outgoing messages |
| `GET` | `/acp/v1/debug/messages` | Same message list, unwrapped, for debugging |
| `POST` | `/acp/v1/messages/send` | Send a message to a peer |
| `POST` | `/acp/v1/messages/{msg_id}/ack` | Acknowledge a message |
| `POST` | `/acp/v1/messages/{msg_id}/error` | Report a processing error |
| `POST` | `/acp/v1/stream/init` | Open a stream for a large reply |

Incoming messages are kept in an in-memory inbox, which is what the dashboard renders when it is pointed at an agent rather than the relay.

## Configuration

Create `acp-peers.yaml` (see `acp-peers-naiplawan.yaml` for a working local example):

```yaml
config_version: 1

this_agent:
  agent_id: "my-agent"
  machine_id: "laptop-1"
  http_endpoint: "https://laptop-1.local:8443/acp/v1"
  ws_endpoint: "wss://laptop-1.local:8443/acp/stream"
  capabilities: []

peers:
  - agent_id: "other-agent"
    machine_id: "server-1"
    http_endpoint: "https://server-1.local:8443/acp/v1"
    ws_endpoint: "wss://server-1.local:8443/acp/stream"
    auth:
      type: "signed-token"
      secret_path: "/etc/acp/secrets/shared.key"

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

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `ACP_PEERS_PATH` | Path to `acp-peers.yaml` |
| `ACP_SHARED_SECRET` | HMAC signing secret |
| `ACP_RELAY_URL` | Cloud relay URL (for signaling) |
| `ACP_AGENT_ID` | This agent's ID |
| `ACP_MACHINE_ID` | This machine's ID |
| `ACP_HTTP_ENDPOINT` | Public URL of this agent |
| `ACP_POLL_INTERVAL` | Relay poll interval (seconds) |
| `ACP_PUBLIC_DEBUG` | Development-only bypass for diagnostics; never enable on a reachable deployment |

## Protocol

See [PROTOCOL.md](./PROTOCOL.md) for full specification.

## Context Handoff (CHP)

ACP-CHP enables rich task delegation between agents:

```rust
use acp_agent::{ACPAgent, chp::build_handoff};

let bundle = build_handoff(
    "Fix login timeout",
    "User can login within 3s",
    "FE-042",
    "Debug token refresh race condition",
    "agent-alpha",
);

agent.send_handoff("agent-beta", bundle).await?;
```

CHP intents: `handshake`, `handoff`, `handover_request/accept/decline`, `progress`, `complete`, `error`
