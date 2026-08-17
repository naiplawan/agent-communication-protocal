FROM node:22-slim

WORKDIR /app

COPY package*.json ./
RUN npm ci

COPY . .

ENV HOST=0.0.0.0
ENV PORT=3000

RUN chown -R node:node /app
USER node

EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
    CMD node -e "require('http').get('http://127.0.0.1:3000/', response => process.exit(response.statusCode === 200 ? 0 : 1)).on('error', () => process.exit(1))"

CMD ["npm", "run", "dev", "--", "--host", "0.0.0.0"]
