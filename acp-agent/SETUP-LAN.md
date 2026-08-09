# Two-Laptop LAN Setup

This setup keeps the ACP relay and dashboard on the first laptop and runs the LLM agent on a second laptop connected to the same Wi-Fi network.

## First Laptop: Relay

Find the Wi-Fi address:

```bash
hostname -I
```

The current relay laptop address is `192.168.1.36`.

Start the relay and dashboard:

```bash
cd acp-server
docker compose up -d relay dashboard
```

Verify LAN reachability from the second laptop:

```bash
curl http://192.168.1.36:8443/health
```

## Second Laptop: LLM Agent

Copy or clone this repository onto the second laptop, then enter `acp-agent/llm-agent`.

Create the local environment file from the template:

```bash
cp .env.remote-agent.example .env.remote-agent
```

Set these values in `.env.remote-agent`:

- `ACP_RELAY_URL`: the first laptop's Wi-Fi address and relay port
- `ACP_MACHINE_ID`: a unique name for the second laptop
- `ACP_HTTP_ENDPOINT`: the second laptop's LAN address and agent port
- `ACP_SHARED_SECRET`: the same secret used by the relay
- `OPENROUTER_API_KEY`: the model API key

Start only the remote agent:

```bash
docker compose -f docker-compose.remote-agent.yml up -d --build
```

Confirm registration:

```bash
docker compose -f docker-compose.remote-agent.yml logs -f
```

Expected output:

```text
[REGISTERED] naiplawan-agent@second-laptop
```

## Test From The First Laptop

The local sender config should target the registered agent through the relay. Send a test task with `acp-peers-opencode.yaml`:

```bash
ACP_SHARED_SECRET="$(cut -d= -f2 ../acp-server/.env)" \
  cargo run --quiet -- \
  --config acp-peers-opencode.yaml \
  send naiplawan-agent '{"message":"Reply with a short greeting and identify your machine."}'
```

Open `http://192.168.1.36:3000/inbox` to inspect the reply.

## Notes

- The LLM agent uses relay polling, so its HTTP endpoint does not need to be port-forwarded for this test.
- Both laptops must allow outbound TCP traffic to port `8443` on the first laptop.
- Guest Wi-Fi isolation can block laptop-to-laptop traffic. Use the same non-guest network if the health check fails.
- The relay's signed-token secret must match on both laptops.
