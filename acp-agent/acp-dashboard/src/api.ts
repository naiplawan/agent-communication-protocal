import type { AgentHealth, CommandCenterMetrics, Dispatch, DispatchFilters, PendingMessage, Peer } from './types';

const API_BASE = import.meta.env.VITE_RELAY_URL || '/api/relay';

async function fetchJson<T>(path: string): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, { headers: { Accept: 'application/json' } });
  if (!response.ok) throw new Error(`Relay API error: ${response.status} ${response.statusText}`);
  return response.json() as Promise<T>;
}

async function postJson<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    method: 'POST',
    headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`Relay API error: ${response.status} ${response.statusText}`);
  return response.json() as Promise<T>;
}

export const getHealth = () => fetchJson<{ status: string; agent: string }>('/health');
export const getAllMessages = () => fetchJson<{ messages: PendingMessage[] }>('/acp/v1/debug/messages');
export const getPeers = () => fetchJson<{ peers: Peer[] }>('/acp/v1/peers');
export const sendMessage = (envelope: unknown, payload: unknown) => postJson<{ msg_id: string; status: string }>('/acp/v1/messages/send', { envelope, payload });

export function transformToDispatch(message: PendingMessage): Dispatch {
  const envelope = message.envelope || {
    msg_id: message.msg_id || 'unknown',
    corr_id: message.corr_id,
    sender: { agent_id: message.sender_agent || 'unknown' },
    recipient: { agent_id: message.recipient_agent || 'unknown' },
    intent: message.intent || 'delegate',
    error: message.error,
  };
  const payload = message.payload;
  const role = (agentId: string) => {
    const value = agentId.split('-')[0].toUpperCase();
    return ['FE', 'BE', 'QA', 'SA', 'PM', 'DEV', 'OPS'].includes(value) ? value : undefined;
  };
  const serialized = payload ? JSON.stringify(payload, null, 2) : undefined;
  const readablePayload = typeof payload?.content === 'string'
    ? payload.content
    : typeof payload?.message === 'string'
      ? payload.message
      : typeof payload?.task === 'string'
        ? payload.task
        : undefined;
  const intent = (envelope.intent || 'delegate') as Dispatch['intent'];
  const statusByIntent: Record<string, Dispatch['status']> = { delegate: 'dispatched', reply: 'acknowledged', ack: 'acknowledged', error: 'blocked', stream_start: 'in_progress', stream_chunk: 'in_progress', stream_end: 'verification' };
  return {
    dispatch_id: envelope.msg_id,
    correlation_id: envelope.corr_id || envelope.msg_id,
    cdf_story_id: payload?.story_id ? String(payload.story_id) : payload?.cdf_story_id ? String(payload.cdf_story_id) : undefined,
    intent,
    from: { agent_id: envelope.sender.agent_id, role: role(envelope.sender.agent_id), machine_id: envelope.sender.machine_id || 'unknown' },
    to: { agent_id: envelope.recipient.agent_id, role: role(envelope.recipient.agent_id), machine_id: envelope.recipient.machine_id || 'unknown' },
    status: statusByIntent[intent] || 'dispatched',
    evidence_status: (payload?.evidence_status as Dispatch['evidence_status']) || 'missing',
    contract_status: (payload?.contract_status as Dispatch['contract_status']) || 'not_checked',
    approval_state: (payload?.approval_state as Dispatch['approval_state']) || 'pending',
    required_approvals: (payload?.required_approvals as string[]) || [],
    received_approvals: (payload?.received_approvals as string[]) || [],
    last_updated: new Date().toISOString(),
    risk: (payload?.risk as Dispatch['risk']) || 'low',
    payload_preview: readablePayload && readablePayload.length > 160 ? `${readablePayload.slice(0, 160)}...` : readablePayload,
    payload_content: serialized,
    deadline: envelope.deadline,
    error: envelope.error,
  };
}

export function filterDispatches(dispatches: Dispatch[], filters: DispatchFilters) {
  return dispatches.filter((item) =>
    (!filters.story || item.cdf_story_id === filters.story) &&
    (!filters.agent || item.from.agent_id === filters.agent || item.to.agent_id === filters.agent) &&
    (!filters.intent || item.intent === filters.intent) &&
    (!filters.status || item.status === filters.status) &&
    (!filters.approval_state || item.approval_state === filters.approval_state) &&
    (!filters.evidence_status || item.evidence_status === filters.evidence_status) &&
    (!filters.contract_drift || item.contract_status === 'drift')
  );
}

export function transformToAgentHealth(peers: Peer[]): AgentHealth[] {
  return peers.map((peer) => ({
    peer,
    status: peer.last_seen_at && Date.now() - peer.last_seen_at * 1000 < 300000 ? 'online' : 'offline',
    queue_depth: 0,
    retry_count: 0,
    is_relay: peer.agent_id.includes('relay'),
    cdf_context_freshness: 'unknown',
    version_compatible: true,
  }));
}

export function calculateMetrics(dispatches: Dispatch[], agents: AgentHealth[]): CommandCenterMetrics {
  const oneDayAgo = Date.now() - 86_400_000;
  return {
    active_stories: new Set(dispatches.filter((d) => ['in_progress', 'verification'].includes(d.status)).map((d) => d.cdf_story_id).filter(Boolean)).size,
    pending_handoffs: dispatches.filter((d) => ['dispatched', 'acknowledged'].includes(d.status)).length,
    blocked_work: dispatches.filter((d) => ['blocked', 'delivery_failed'].includes(d.status)).length,
    awaiting_approvals: dispatches.filter((d) => d.approval_state === 'required').length,
    contract_drift: dispatches.filter((d) => d.contract_status === 'drift').length,
    unverified_evidence: dispatches.filter((d) => ['missing', 'stale'].includes(d.evidence_status)).length,
    offline_agents: agents.filter((a) => a.status === 'offline').length,
    recent_completions: dispatches.filter((d) => d.status === 'complete' && new Date(d.last_updated).getTime() > oneDayAgo).length,
  };
}
