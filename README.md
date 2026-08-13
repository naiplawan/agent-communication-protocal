# ACP

ACP (Agent Communication Protocol) is a self-hosted request/reply protocol for delegating work between agents on different machines. Messages use JSON envelopes, correlation IDs, hop-by-hop acknowledgements, signed authentication, and a reply path that routes results back through the delegation chain.

## Repository layout

```text
acp-agent/
├── acp-core/           Shared Rust protocol types
├── acp-agent/          Rust agent CLI and library
├── acp-dashboard/      Web dashboard (Vite + React + TypeScript)
├── acp-peers-naiplawan.yaml  Example peer config (local agent + relay peer)
├── PROTOCOL.md         Protocol specification
├── SETUP.md            Standard setup guide
└── SETUP-LAN.md        Two-laptop LAN setup guide

acp-server/
├── relay-server/       Rust relay with SQLite persistence
├── docker-compose.yml  Relay + dashboard development stack
└── Dockerfile.relay    Relay server container
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
cp .env.example .env  # or create manually with ACP_SHARED_SECRET
docker compose up --build
```

The relay listens on port `8443` and persists messages in the `acp-relay-data` Docker volume. The relay dashboard is available at `http://localhost:3001` (host port `3000` is already in use on this machine).

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

## Agent HTTP API

A running agent exposes:

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/health` | Status plus this agent's `this_agent_id` / `this_machine_id` |
| `GET` | `/acp/v1/peers` | Peers from the loaded config |
| `GET` | `/acp/v1/messages/pending` | Inbox and pending outgoing messages |
| `GET` | `/acp/v1/debug/messages` | Same message list, unwrapped, for debugging |
| `POST` | `/acp/v1/messages/send` | Send a message to a peer |
| `POST` | `/acp/v1/messages/{msg_id}/ack` | Acknowledge a message |
| `POST` | `/acp/v1/messages/{msg_id}/error` | Report a processing error |
| `POST` | `/acp/v1/stream/init` | Open a stream for a large reply |

The dashboard talks to these endpoints, so it can point at either an agent or the relay.

## Dashboard

The dashboard reads its backend URL from `VITE_RELAY_URL`, which is baked in at build time:

```bash
cd acp-agent/acp-dashboard
VITE_RELAY_URL=http://localhost:8444 npm run build
```

In Docker the same value is passed as a build argument (`docker compose` sets it to `http://relay:8443`). When it points at an agent, the dashboard shows that agent's identity and its own inbox instead of relay-wide traffic.

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

## Development status

ACP is an active beta implementation. The protocol and relay paths are present, but some CLI and deployment paths are still evolving.

## License

MIT License
