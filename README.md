# ACP

ACP (Agent Communication Protocol) is a self-hosted request/reply protocol for delegating work between agents on different machines. Messages use JSON envelopes, correlation IDs, hop-by-hop acknowledgements, signed authentication, and a reply path that routes results back through the delegation chain.

## Repository layout

```text
acp-agent/
├── acp-core/          Shared Rust protocol types
├── acp-agent/         Rust agent CLI and library
├── deploy/            Friend/agent deployment files
├── PROTOCOL.md        Protocol specification
└── SETUP.md           Two-machine setup guide

acp-server/
├── relay-server/       Rust relay with SQLite persistence
├── docker-compose.yml  Relay + mock-agent development stack
└── Dockerfile         Relay server container
```

## How it works

```text
Human → Agent A → Relay/Agent B → Agent C
         ↑______________________________|
                 reply path
```

Each message contains an `origin`, `sender`, `recipient`, `corr_id`, and ordered `reply_to.path`. An agent only needs to know its immediate peers; replies are forwarded hop by hop. The protocol supports delegation, replies, errors, acknowledgements, and WebSocket stream frames.

## Requirements

- Rust toolchain (for the agent and relay)
- Docker and Docker Compose (for the containerized relay setup)
- A shared HMAC secret for authenticated machines

## Quick start: relay with Docker

From `acp-server/`:

```bash
export ACP_SHARED_SECRET="$(openssl rand -hex 32)"
docker compose up --build
```

The relay listens on port `8443` and persists messages in the `acp-relay-data` Docker volume. The compose stack also starts a mock agent for local testing.

Check health:

```bash
curl http://localhost:8443/health
```

## Build and use the Rust agent

```bash
cd acp-agent
cargo build --release

# Start an agent
./target/release/acp-agent --config /path/to/acp-peers.yaml run --port 8443

# Send a JSON message
./target/release/acp-agent --config /path/to/acp-peers.yaml \
  send agent-beta '{"task":"ping","echo":"hello"}'

# Inspect configuration and a peer
./target/release/acp-agent --config /path/to/acp-peers.yaml doctor agent-beta
```

The Rust CLI also accepts `run --use-signaling` when relay signaling is configured through the environment.

## Configuration

Peer configuration is YAML. A minimal shape is:

```yaml
config_version: 1
this_agent:
  agent_id: agent-alpha
  machine_id: laptop-1
  http_endpoint: http://localhost:8443/acp/v1
  ws_endpoint: ws://localhost:8443/acp/stream
peers:
  - agent_id: agent-beta
    machine_id: server-1
    http_endpoint: https://server-1.example.com:8443/acp/v1
    ws_endpoint: wss://server-1.example.com:8443/acp/stream
    auth:
      type: signed-token
      secret_path: /etc/acp/shared-secret.key
```

Use HTTPS/WSS outside a trusted local network. The protocol supports HMAC-signed tokens by default and mTLS for higher-security deployments.

## Documentation

- [Protocol specification](acp-agent/PROTOCOL.md)
- [Two-machine setup](acp-agent/SETUP.md)
- [Agent skill/integration guide](acp-agent/SKILL.md)
- [Friend deployment guide](acp-agent/deploy/README.md)

## Development status

ACP is an active beta implementation. The protocol and relay paths are present, but some CLI and deployment paths are still evolving.

## License

MIT License
