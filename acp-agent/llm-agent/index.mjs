import crypto from 'node:crypto';
import { OpenRouter } from '@openrouter/sdk';

const relay = process.env.ACP_RELAY_URL || 'http://localhost:8443';
const agentId = process.env.ACP_AGENT_ID || 'naiplawan-agent';
const machineId = process.env.ACP_MACHINE_ID || 'naiplawan-machine';
const secret = process.env.ACP_SHARED_SECRET;
const model = process.env.OPENROUTER_MODEL || 'deepseek/deepseek-v4-flash-0731';
const pollInterval = Number(process.env.ACP_POLL_INTERVAL || 3) * 1000;

if (!secret) throw new Error('ACP_SHARED_SECRET must be set');
if (!process.env.OPENROUTER_API_KEY) throw new Error('OPENROUTER_API_KEY must be set');

const openrouter = new OpenRouter({ apiKey: process.env.OPENROUTER_API_KEY });

function encode(value) {
  return Buffer.from(value).toString('base64url');
}

function token(audienceAgent, audienceMachine, msgId) {
  const header = encode(JSON.stringify({ alg: 'HS256', typ: 'ACP' }));
  const now = Date.now();
  const payload = encode(JSON.stringify({
    iss: `${agentId}@${machineId}`,
    aud: `${audienceAgent}@${audienceMachine}`,
    exp: new Date(now + 3600000).toISOString(),
    iat: new Date(now).toISOString(),
    msg_id: msgId,
    nonce: crypto.randomUUID(),
  }));
  const input = `${header}.${payload}`;
  const signature = crypto.createHmac('sha256', Buffer.from(secret, 'hex')).update(input).digest('base64url');
  return `${input}.${signature}`;
}

async function request(path, options = {}) {
  const response = await fetch(`${relay}${path}`, options);
  if (!response.ok) throw new Error(`${response.status} ${await response.text()}`);
  return response.json();
}

async function register() {
  await request('/acp/v1/agents/register', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      agent_id: agentId,
      machine_id: machineId,
      http_endpoint: `http://${agentId}:8444`,
      capabilities: ['llm', 'openrouter', 'streaming'],
    }),
  });
  console.log(`[REGISTERED] ${agentId}@${machineId}`);
}

async function poll() {
  const msgId = `poll_${crypto.randomUUID()}`;
  return request('/acp/v1/messages/pending', {
    headers: { Authorization: `ACP-Token ${token('acp-relay', 'relay', msgId)}` },
  });
}

async function acknowledge(message) {
  const msgId = message.envelope.msg_id;
  await request(`/acp/v1/messages/${msgId}/ack`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      Authorization: `ACP-Token ${token('acp-relay', 'relay', msgId)}`,
    },
    body: JSON.stringify({
      ack_type: 'hop_ack',
      received: true,
      processed: true,
    }),
  });
}

function messageText(payload) {
  if (typeof payload === 'string') return payload;
  if (!payload || typeof payload !== 'object') return JSON.stringify(payload ?? {});
  const parts = [];
  if (payload.message) parts.push(`Message:\n${String(payload.message)}`);
  if (payload.content) parts.push(`Content:\n${typeof payload.content === 'string' ? payload.content : JSON.stringify(payload.content)}`);
  if (payload.link) parts.push(`Link:\n${String(payload.link)}`);
  const attachment = payload.attachment;
  if (attachment?.name) {
    const metadata = `Attachment: ${attachment.name} (${attachment.type || 'unknown type'}, ${attachment.size || 0} bytes)`;
    const encoded = typeof attachment.data === 'string' ? attachment.data.split(',')[1] : null;
    if (encoded && String(attachment.type || '').startsWith('text/')) {
      const content = Buffer.from(encoded, 'base64').toString('utf8');
      parts.push(`${metadata}\nAttachment content:\n${content.slice(0, 100000)}`);
    } else {
      parts.push(metadata);
    }
  }
  return parts.join('\n\n') || JSON.stringify(payload);
}

function parseAgentAddress(address) {
  const separator = address.lastIndexOf('@');
  if (separator < 1) return null;
  return {
    agent_id: address.slice(0, separator),
    machine_id: address.slice(separator + 1),
  };
}

async function generateReply(text) {
  const stream = await openrouter.chat.send({
    chatRequest: {
      model,
      messages: [{ role: 'user', content: text }],
      stream: true,
    },
  });
  let response = '';
  for await (const chunk of stream) {
    const content = chunk.choices?.[0]?.delta?.content;
    if (content) response += content;
  }
  return response || 'The LLM returned an empty response.';
}

async function replyTo(message, content) {
  const incoming = message.envelope;
  const replyId = `msg_${crypto.randomUUID().replaceAll('-', '').slice(0, 12)}`;
  const replyPath = incoming.reply_to?.path || [];
  const recipient = parseAgentAddress(replyPath.at(-1)) || incoming.origin || incoming.sender;

  if (recipient.agent_id === agentId && recipient.machine_id === machineId) {
    console.log(`[SKIPPED] Not replying to self for ${incoming.msg_id}`);
    return;
  }

  const envelope = {
    msg_id: replyId,
    corr_id: incoming.corr_id,
    origin: incoming.origin || incoming.sender,
    sender: { agent_id: agentId, machine_id: machineId },
    recipient,
    reply_to: { path: replyPath },
    hops: { count: 0, max: 10, trace: [] },
    intent: 'reply',
    content_type: 'application/json',
    priority: 'normal',
    deadline: null,
  };
  await request('/acp/v1/messages/send', {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      Authorization: `ACP-Token ${token(recipient.agent_id, recipient.machine_id || '', replyId)}`,
    },
    body: JSON.stringify({ envelope, payload: { content } }),
  });
  console.log(`[REPLIED] ${replyId} -> ${recipient.agent_id}`);
}

async function main() {
  await register();
  const processed = new Set();
  for (;;) {
    try {
      const { messages } = await poll();
      await Promise.all((messages || []).map(async (message) => {
        const incomingId = message.envelope.msg_id;
        if (processed.has(incomingId)) return;
        processed.add(incomingId);
        const text = messageText(message.payload);
        console.log(`[RECEIVED] ${incomingId} from ${message.envelope.sender.agent_id}`);
        const response = await generateReply(text);
        await replyTo(message, response);
        await acknowledge(message);
      }));
    } catch (error) {
      console.error(`[ERROR] ${error.message}`);
      await new Promise((resolve) => setTimeout(resolve, pollInterval));
    }
    await new Promise((resolve) => setTimeout(resolve, pollInterval));
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
