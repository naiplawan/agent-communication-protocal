FROM python:3.12-slim

WORKDIR /app

# Install dependencies
COPY pyproject.toml README.md ./
RUN pip install --no-cache-dir flask flask-socketio httpx pyyaml python-socketio[aiohttp] aiohttp

# Copy server/ as src/acp_server/ (matches pyproject.toml package layout)
RUN mkdir -p src
COPY server/ src/acp_server/

ENV PYTHONUNBUFFERED=1
ENV PYTHONPATH=/app/src
ENV ACP_SERVER_CONFIG=/app/acp-peers.yaml

# Install acp-server in development mode
RUN pip install --no-cache-dir -e .

CMD ["acp-server"]

EXPOSE 8443
