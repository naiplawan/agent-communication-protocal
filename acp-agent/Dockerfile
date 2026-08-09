FROM rust:1.93-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY acp-core ./acp-core
COPY acp-agent ./acp-agent
RUN cargo build --release -p acp-agent

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/acp-agent /usr/local/bin/acp-agent
COPY acp-peers-naiplawan.yaml /etc/acp/acp-peers.yaml

ENV ACP_PEERS_PATH=/etc/acp/acp-peers.yaml \
    ACP_RELAY_URL=http://relay:8443 \
    ACP_AGENT_ID=naiplawan-agent \
    ACP_MACHINE_ID=naiplawan-machine \
    ACP_HTTP_ENDPOINT=http://naiplawan-agent:8444 \
    ACP_POLL_INTERVAL=3

EXPOSE 8444

CMD ["acp-agent", "run", "--port", "8444", "--use-signaling"]
