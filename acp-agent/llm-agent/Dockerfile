FROM node:22-slim

WORKDIR /app

COPY package.json ./
RUN npm install --omit=dev

COPY index.mjs ./

ENV ACP_RELAY_URL=http://relay:8443 \
    ACP_AGENT_ID=naiplawan-agent \
    ACP_MACHINE_ID=naiplawan-machine \
    ACP_POLL_INTERVAL=3 \
    OPENROUTER_MODEL=deepseek/deepseek-v4-flash-0731

CMD ["node", "index.mjs"]
