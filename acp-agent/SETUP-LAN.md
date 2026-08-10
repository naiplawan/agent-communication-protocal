# Two-Laptop LAN Setup

This setup keeps the ACP relay and dashboard on the first laptop and runs an agent on a second laptop connected to the same Wi-Fi network.

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

## Second Laptop: Agent

Copy or clone this repository onto the second laptop, then enter `acp-agent`.

Start the agent in signaling mode so it registers with the relay and polls it for work:

```bash
ACP_RELAY_URL=http://192.168.1.36:8443 \
ACP_AGENT_ID=naiplawan-agent \
ACP_MACHINE_ID=second-laptop \
ACP_HTTP_ENDPOINT=http://<second-laptop-ip>:8444 \
ACP_SHARED_SECRET=<same secret as the relay> \
  cargo run --release -- run --port 8444 --use-signaling
```

- `ACP_RELAY_URL`: the first laptop's Wi-Fi address and relay port
- `ACP_MACHINE_ID`: a unique name for the second laptop
- `ACP_HTTP_ENDPOINT`: the second laptop's LAN address and agent port
- `ACP_SHARED_SECRET`: the same secret used by the relay

Expected output:

```text
[REGISTERED] naiplawan-agent@second-laptop
```

## Test From The First Laptop

The local sender config should target the registered agent through the relay. Send a test task with `acp-peers-naiplawan.yaml`:

```bash
ACP_SHARED_SECRET="$(cut -d= -f2 ../acp-server/.env)" \
  cargo run --quiet -- \
  --config acp-peers-naiplawan.yaml \
  send naiplawan-agent '{"message":"Reply with a short greeting and identify your machine."}'
```

Open `http://192.168.1.36:3000/inbox` to inspect the reply.

## Notes

- The remote agent uses relay polling, so its HTTP endpoint does not need to be port-forwarded for this test.
- Both laptops must allow outbound TCP traffic to port `8443` on the first laptop.
- Guest Wi-Fi isolation can block laptop-to-laptop traffic. Use the same non-guest network if the health check fails.
- The relay's signed-token secret must match on both laptops.
