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
└── acp-agent/     # Agent binary + CLI
    └── src/
        ├── main.rs        # CLI entry point
        ├── agent.rs       # ACPAgent struct
        ├── server.rs      # HTTP server
        └── signaling.rs   # Relay client
```

## Features

- **Multi-hop delegation** with reply path routing
- **ACP-CHP** (Context Handoff Protocol) for rich task context transfer
- **HMAC-SHA256 signed tokens** for peer authentication
- **Exponential backoff retry** for reliable message delivery
- **WebSocket streaming** for large replies
- **Relay support** for NAT traversal

## Build

```bash
cargo build --release
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

## Configuration

Create `acp-peers.yaml`:

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
