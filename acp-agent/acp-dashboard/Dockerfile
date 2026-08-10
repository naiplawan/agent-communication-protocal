FROM node:22-slim

WORKDIR /app

COPY package*.json ./
RUN npm ci

COPY . .

ARG VITE_RELAY_URL=http://localhost:8444
ENV VITE_RELAY_URL=$VITE_RELAY_URL

ENV HOST=0.0.0.0
ENV PORT=3000

EXPOSE 3000

CMD ["npm", "run", "dev", "--", "--host", "0.0.0.0"]
