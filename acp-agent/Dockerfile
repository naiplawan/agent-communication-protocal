# The builder is pinned to bookworm to match the runtime stage. The floating
# `rust:1.93-slim` tag now resolves to Debian 13 (trixie), which would link the
# binary against OpenSSL 3.5 and glibc 2.41 — neither of which bookworm has.
FROM rust:1.93-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY acp-core ./acp-core
COPY acp-agent ./acp-agent
# Avoid BuildKit-only cache mounts so this image builds with a standard Docker
# installation as well.
RUN cargo build --release -p acp-agent && \
    cp /build/target/release/acp-agent /usr/local/bin/acp-agent

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash acp

COPY --from=builder /usr/local/bin/acp-agent /usr/local/bin/acp-agent
COPY acp-peers.yaml /etc/acp/acp-peers.yaml

ENV ACP_PEERS_PATH=/etc/acp/acp-peers.yaml \
    ACP_RELAY_URL=http://relay:8443 \
    ACP_AGENT_ID=naiplawan-agent \
    ACP_MACHINE_ID=naiplawan-machine \
    ACP_HTTP_ENDPOINT=http://naiplawan-agent:8444 \
    ACP_POLL_INTERVAL=3

EXPOSE 8444

USER acp

CMD ["acp-agent", "run", "--port", "8444", "--use-signaling"]
