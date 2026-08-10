---
name: acp
description: Agent Communication Protocol — real-time request/reply for self-hosted agent delegation across machines. Use when building multi-agent meshes where agents delegate to each other with native reply routing.
---

# ACP — Agent Communication Protocol Skill

A real-time request/reply protocol enabling **Human → Agent → Agent → Human** chains across machines, with native multi-hop reply routing and human-in-the-loop approval gates.

## Full Documentation

- **[PROTOCOL.md](PROTOCOL.md)** — Source of truth: message envelope format, HTTP endpoints, WebSocket protocol, ack/retry mechanics, state machine, security model
- **[SETUP.md](SETUP.md)** — Standard two-machine setup guide
- **[SETUP-LAN.md](SETUP-LAN.md)** — Two-laptop LAN setup guide
- **[README.md](README.md)** — Architecture overview, quick start, file structure, dependencies

## When to Use This

Use ACP when all three hold:

1. **Work crosses a machine boundary** — agents are on different hosts
2. **No direct network reachability** — HTTPS with NAT traversal needed
3. **Minutes-to-hours response time is acceptable** — synchronous blocking via HTTP

Do NOT use ACP for:
- Same-machine parallelism (use subagents instead — faster, no network)
- Real-time/streaming needs beyond WebSocket chunking (use a message queue)
- When agents can reach each other directly over LAN (raw HTTP is simpler)

## Core Concept

Every message carries a `reply_to.path` — an ordered list of agents. When Agent B receives a message from Agent A and decides to delegate to Agent C, it:
1. Appends itself to `reply_to.path`
2. Sets `recipient = agent-c`
3. Forwards the message

When Agent C replies, it sends the reply to the last entry in `reply_to.path` (Agent B). Agent B forwards to the previous entry (Agent A). And so on back to the origin.

No agent needs a global view — only local knowledge of peers.

## Human-in-the-Loop Workflow

Messages sent by another human include `origin.human = "<name_or_email>"`. When a human receives such a message, the agent **must not act autonomously** — instead:

1. The human opens the **Inbox** page in the web UI (`http://localhost:3000/inbox`)
2. Clicks the message to open **Message Detail**
3. Clicks **Accept / Blocked / In Progress** to draft a reply
4. The agent sends the human-approved response

The agent never executes a task from another human without this approval gate.

## Web UI (Dashboard)

The React dashboard at `http://localhost:3000` provides:

| Route | Purpose |
|-------|---------|
| `/` | Dashboard — relay health, pending count, recent messages |
| `/inbox` | Inbox with intent filtering and human reply |
| `/compose` | Send a new message or reply to an existing `corr_id` |

**Environment variables** (or `acp-peers.yaml`):

| Variable | Description |
|----------|-------------|
| `ACP_RELAY_URL` | Cloud relay URL (e.g. `http://localhost:8443`) |
| `ACP_AGENT_ID` | This agent's identifier |
| `ACP_MACHINE_ID` | This machine's identifier |
| `ACP_SHARED_SECRET` | Secret for HMAC-SHA256 token signing |
| `ACP_PEERS_PATH` | Path to `acp-peers.yaml` (optional) |

## CLI Tools

| Command | Purpose |
|---------|---------|
| `acp-agent send <target> '<json>'` | Send a message to a peer |
| `acp-agent listen` | Long-poll for incoming messages |
| `acp-agent doctor [--target <agent>]` | Diagnose connectivity and config |
| `acp-agent run [--port <port>]` | Start the agent server |

## Rust Agent Integration

```rust
use acp_agent::{ACPAgent, chp::build_handoff};

let agent = ACPAgent::new("acp-peers.yaml").await?;

let bundle = build_handoff(
    task_title,
    acceptance_criteria,
    ticket_id,
    instructions,
    delegated_by,
);

agent.send_handoff("agent-beta", bundle).await?;
```

## Remote Agent Integration

A remote agent behind NAT registers with the relay and polls it for work:

```bash
ACP_RELAY_URL=http://<relay-host>:8443 \
ACP_AGENT_ID=my-agent \
ACP_MACHINE_ID=my-machine \
ACP_SHARED_SECRET=<secret> \
cargo run --release -- run --port 8444 --use-signaling
```

Environment variables:
- `ACP_RELAY_URL` — relay URL
- `ACP_AGENT_ID` — this agent's ID
- `ACP_MACHINE_ID` — this machine's ID
- `ACP_SHARED_SECRET` — HMAC signing secret
- `ACP_HTTP_ENDPOINT` — public URL of this agent

## Security

- **Signed tokens** (HMAC-SHA256) by default — set `ACP_SHARED_SECRET` env var on all machines
- **mTLS** for high-security environments — configure per-peer in `acp-peers.yaml`
- **No auth** if using plain HTTP — trusted network only

## Protocol Version

**1.0** — Stable. Adding new `intent` values is backward-compatible. Protocol version negotiation available via `GET /acp/v1/capabilities` if needed.
