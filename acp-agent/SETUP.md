# ACP Setup — Two-Machine Agent Mesh

This guide sets up two machines (A and B) so their agents can delegate to each other using ACP over HTTPS.

---

## Overview

```
Machine A                          Machine B
┌─────────────────────┐           ┌─────────────────────┐
│  agent-alpha        │◄─────────►│  agent-beta         │
│  laptop-1           │  HTTP    │  server-2           │
│  :8443              │           │  :8443               │
└─────────────────────┘           └─────────────────────┘
     ▲                                  ▲
     │ shared signing secret            │ shared signing secret
     └──────────────────────────────────┘
```

Each machine needs:
1. ACP config (`acp-peers.yaml`)
2. Shared signing secret
3. The `acp-server` running (see `acp-server/`)

---

## Prerequisites

- Python 3.10+
- `pip install acp-server` (or use Docker)
- Network connectivity between machines on port 8443

> **Same LAN?** Machines reach each other directly via local IP (e.g. `http://192.168.1.10:8443`). No tunneling needed.
>
> **Cross-network (NAT)?** Use [ngrok](#cross-network-setup-with-ngrok) or a VPN (tailscale, WireGuard) to expose port 8443.

---

## Step 1 — Generate Shared Secret

On **either** machine, generate a signing key:

```bash
openssl rand -hex 32 > /etc/acp/shared-secret.key
chmod 600 /etc/acp/shared-secret.key
```

Copy this file to the **same path** on Machine B:

```bash
# From Machine A:
scp /etc/acp/shared-secret.key user@machine-b:/etc/acp/shared-secret.key
```

Or use the `ACP_SHARED_SECRET` environment variable directly (set via `.env` or CI secrets).

---

## Step 2 — Create Config on Machine A

```bash
# On Machine A:
sudo mkdir -p /etc/acp
```

Create `/etc/acp/acp-peers.yaml` on Machine A:

```yaml
config_version: 1
updated_at: "2026-08-06T00:00:00Z"

this_agent:
  agent_id: "agent-alpha"
  machine_id: "laptop-1"
  http_endpoint: "https://laptop-1.local:8443/acp/v1"
  ws_endpoint: "wss://laptop-1.local:8443/acp/stream"
  capabilities:
    - frontend
    - code-authoring

peers:
  - agent_id: "agent-beta"
    machine_id: "server-2"
    http_endpoint: "https://server-2.local:8443/acp/v1"
    ws_endpoint: "wss://server-2.local:8443/acp/stream"
    capabilities:
      - backend
      - database
    auth:
      type: "signed-token"
      secret_path: "/etc/acp/shared-secret.key"

security:
  default_auth_type: "signed-token"
  token_ttl_seconds: 3600
  require_https: true
  min_tls_version: "1.3"

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

## Step 3 — Create Config on Machine B

```yaml
config_version: 1
updated_at: "2026-08-06T00:00:00Z"

this_agent:
  agent_id: "agent-beta"
  machine_id: "server-2"
  http_endpoint: "https://server-2.local:8443/acp/v1"
  ws_endpoint: "wss://server-2.local:8443/acp/stream"
  capabilities:
    - backend
    - database

peers:
  - agent_id: "agent-alpha"
    machine_id: "laptop-1"
    http_endpoint: "https://laptop-1.local:8443/acp/v1"
    ws_endpoint: "wss://laptop-1.local:8443/acp/stream"
    capabilities:
      - frontend
      - code-authoring
    auth:
      type: "signed-token"
      secret_path: "/etc/acp/shared-secret.key"

security:
  default_auth_type: "signed-token"
  token_ttl_seconds: 3600
  require_https: true
  min_tls_version: "1.3"

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

## Step 4 — Set Environment Variable

On **both** machines, set the shared secret env var:

```bash
export ACP_SHARED_SECRET=$(cat /etc/acp/shared-secret.key)
# Add to ~/.bashrc or ~/.zshrc for permanence
echo 'export ACP_SHARED_SECRET=$(cat /etc/acp/shared-secret.key)' >> ~/.bashrc
```

---

## Step 5 — Run the Server

### Option A — Python package (recommended)

```bash
pip install acp-server

# Machine A:
acp-server --config /etc/acp/acp-peers.yaml

# Machine B (same command — uses its own this_agent):
acp-server --config /etc/acp/acp-peers.yaml
```

### Option B — Docker

```bash
# From acp-server/ directory:
docker compose up --build
```

---

## Step 6 — Verify Connectivity

On **Machine A**, run `acp-doctor`:

```bash
./bin/acp-doctor --probe agent-beta
```

Expected output:
```
ACP Doctor — Connectivity Diagnostic
====================================

Config file loads... ✓ OK
this_agent is configured... ✓ OK
  agent_id=agent-alpha
  machine_id=laptop-1
  http_endpoint=https://laptop-1.local:8443/acp/v1
  ws_endpoint=wss://laptop-1.local:8443/acp/stream
Peers are reachable... ✓ OK
  agent-beta@server-2: reachable
Auth token creation/verification... ✓ OK
  Token verified: iss=agent-alpha@laptop-1
Probe message to peer... ✓ OK
  Sent msg_xxx to agent-beta
  Response: {"msg_id": "msg_xxx", "status": "accepted", ...}

====================================
All checks passed
```

---

## Step 7 — Test Delegation

From **Machine A**, send a task to agent-beta:

```bash
./bin/acp-send \
  --to agent-beta \
  --payload '{"task": "ping", "echo": "hello from alpha"}'
```

You should see agent-beta receive it, process it, and send a reply back.

---

## Docker Dry-Run (local)

From `acp-server/`:

```bash
cp ../Skills/acp/.env.example ../Skills/acp/.env
docker compose up --build
```

This starts two containers (alpha + beta) on the same Docker network, communicating over HTTP.

---

## Cross-Network Setup (with ngrok)

If Machine A and B are on different networks, use ngrok to expose port 8443:

```bash
# On Machine A:
ngrok http 8443 --url alpha-agency.ngrok.io

# On Machine B:
ngrok http 8443 --url beta-server.ngrok.io
```

Update `http_endpoint` and `ws_endpoint` in both configs to use the ngrok URLs.

---

## Adding a Third Machine (Machine C)

On Machine A and B, add Machine C to `peers` with its endpoint. No changes to existing peers needed — address-based routing means agents only need to know their direct peers.

---

## Cloud Relay Deployment

Deploy ACP server on a cloud VM so agents behind NAT or on different networks can communicate through the relay.

### Cloud Server Setup

```bash
# On the cloud VM:
pip install acp-server

# Create a cloud config
sudo mkdir -p /etc/acp
```

Create `/etc/acp/acp-cloud.yaml` on the cloud server:

```yaml
config_version: 1
updated_at: "2026-08-07T00:00:00Z"

this_agent:
  agent_id: "acp-cloud-relay"
  machine_id: "cloud-vm-1"
  http_endpoint: "https://acp-cloud.example.com:8443/acp/v1"
  ws_endpoint: "wss://acp-cloud.example.com:8443/acp/stream"
  capabilities:
    - relay
    - registry

peers: []

security:
  default_auth_type: "signed-token"
  token_ttl_seconds: 3600
  require_https: true

timeouts:
  hop_ack_ms: 5000
  process_ack_ms: 300000
  stream_init_ms: 10000
```

Run with:

```bash
export ACP_SHARED_SECRET=$(cat /etc/acp/shared-secret.key)
acp-server --config /etc/acp/acp-cloud.yaml --port 8443
```

### Agent Configuration (All Agents)

Each agent only needs the cloud server as its peer:

```yaml
# Machine A config
this_agent:
  agent_id: "agent-alpha"
  machine_id: "laptop-1"
  http_endpoint: "https://laptop-1.dyndns.com:8443/acp/v1"  # or any public URL
  ws_endpoint: "wss://laptop-1.dyndns.com:8443/acp/stream"
  capabilities: ["frontend", "code-authoring"]

peers:
  - agent_id: "acp-cloud-relay"
    machine_id: "cloud-vm-1"
    http_endpoint: "https://acp-cloud.example.com:8443/acp/v1"
    ws_endpoint: "wss://acp-cloud.example.com:8443/acp/stream"
    auth:
      type: "signed-token"
      secret_path: "/etc/acp/shared-secret.key"

security:
  default_auth_type: "signed-token"
  token_ttl_seconds: 3600
  require_https: true
```

### How It Works

```
Agent Alpha                          Cloud Relay                        Agent Beta
    │                                    │                                   │
    │──── POST /messages/send ──────────>│                                   │
    │     recipient: agent-beta          │                                   │
    │                                    │──── POST /messages/send ─────────>│
    │                                    │     (cloud relay forwards)         │
    │                                    │                                   │
    │<─── 202 Accepted ──────────────────│                                   │
    │                                    │<─── 202 Accepted ─────────────────│
    │                                    │                                   │
    │                                    │    [Agent Beta processes task]    │
    │                                    │                                   │
    │                                    │<─── ACK ──────────────────────────│
    │<─── ACK (via WebSocket/Polling) ───│                                   │
```

The cloud relay acts as a **message broker**:
1. Agents send messages to the relay (which stores them)
2. The relay forwards to the recipient when they poll or connect via WebSocket
3. Replies follow the same path in reverse

### No ngrok Needed

Since the cloud server is publicly reachable, agents only need **outbound HTTPS** to the relay — no inbound ports required on the agent machines.

---

## Uninstalling / Resetting

1. Stop the running servers
2. Delete `/etc/acp/` (configs and secrets)
3. Optionally `pip uninstall acp-server`
