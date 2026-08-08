---
name: acp
description: Agent Communication Protocol — real-time request/reply for self-hosted agent delegation across machines. Use when building multi-agent meshes where agents delegate to each other with native reply routing.
---

# ACP — Agent Communication Protocol Skill

A real-time request/reply protocol enabling **Human → Agent → Agent → Human** chains across machines, with native multi-hop reply routing and human-in-the-loop approval gates.

## Full Documentation

- **[PROTOCOL.md](PROTOCOL.md)** — Source of truth: message envelope format, HTTP endpoints, WebSocket protocol, ack/retry mechanics, state machine, security model
- **[SETUP.md](SETUP.md)** — Step-by-step two-machine setup guide
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

1. The human opens the **Messages** page in the web UI (`http://localhost:8080/messages`)
2. Clicks the message to open **Message Detail**
3. Clicks **Accept / Blocked / In Progress** to draft a reply
4. The agent sends the human-approved response

The agent never executes a task from another human without this approval gate.

## Web UI (Agent Side)

The Flask web UI at `http://localhost:8080` provides:

| Route | Purpose |
|-------|---------|
| `/` | Dashboard — relay health, pending count, recent messages |
| `/messages` | Inbox / Outbox with intent filtering |
| `/messages/<msg_id>` | Full envelope + payload, human reply section |
| `/compose` | Send a new message or reply to an existing `corr_id` |
| `/delegation` | Track delegated tasks and their delivery status |

**Environment variables** (or `acp-peers.yaml`):

| Variable | Description |
|----------|-------------|
| `ACP_RELAY_URL` | Cloud relay URL (e.g. `https://relay.example.com:8443`) |
| `ACP_AGENT_ID` | This agent's identifier |
| `ACP_MACHINE_ID` | This machine's identifier |
| `ACP_SHARED_SECRET` | Secret for HMAC-SHA256 token signing |
| `ACP_PEERS_PATH` | Path to `acp-peers.yaml` (optional) |

## Tools

| Tool | Purpose |
|------|---------|
| `bin/acp-send` | Send a message to a peer |
| `bin/acp-listen` | Long-poll for incoming messages (simple fallback) |
| `bin/acp-ack` | Acknowledge a message |
| `bin/acp-doctor` | Diagnose connectivity and config |

## Agent Integration

```python
from lib.agent import ACPAgent

agent = ACPAgent(config_path="/etc/acp/acp-peers.yaml")

@agent.on_delegate
def handle(msg):
    # msg.envelope has origin, sender, reply_to.path
    # msg.payload has the task data
    # If origin.human is set, MUST prompt human before acting
    if msg.envelope.origin.human:
        return agent.escalate_to_human(msg)
    return {"result": "done", "findings": [...]}

agent.run()  # starts Flask server on :8443
```

## Security

- **Signed tokens** (HMAC-SHA256) by default — set `ACP_SHARED_SECRET` env var on all machines
- **mTLS** for high-security environments — configure per-peer in `acp-peers.yaml`
- **No auth** if using plain HTTP — trusted network only

## Protocol Version

**1.0** — Stable. Adding new `intent` values is backward-compatible. Protocol version negotiation available via `GET /acp/v1/capabilities` if needed.
