import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';
import { createHmac, randomUUID } from 'crypto';

function dashboardToken(secret: string) {
  const encode = (value: string) => Buffer.from(value).toString('base64url');
  const header = encode(JSON.stringify({ alg: 'HS256', typ: 'ACP' }));
  const now = Math.floor(Date.now() / 1000);
  const payload = encode(JSON.stringify({
    iss: 'dashboard@local', aud: 'acp-relay@relay',
    exp: new Date((now + 3600) * 1000).toISOString(),
    iat: new Date(now * 1000).toISOString(),
    msg_id: `dashboard_${randomUUID()}`, nonce: randomUUID(),
  }));
  const input = `${header}.${payload}`;
  const key = /^[0-9a-f]+$/i.test(secret) && secret.length % 2 === 0
    ? Buffer.from(secret, 'hex')
    : Buffer.from(secret, 'utf8');
  const signature = createHmac('sha256', key).update(input).digest('base64url');
  return `${input}.${signature}`;
}

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    port: 3000,
    proxy: {
      '/api/relay': {
        target: process.env.ACP_RELAY_URL || 'http://localhost:8443',
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api\/relay/, ''),
        configure: (proxy) => {
          proxy.on('proxyReq', (request) => {
            const secret = process.env.ACP_SHARED_SECRET;
            if (secret) request.setHeader('Authorization', `ACP-Token ${dashboardToken(secret)}`);
          });
        },
      },
    },
  },
});
