// ACP Protocol Types (mirroring Rust backend models)

export interface AgentAddr {
  agent_id: string;
  machine_id?: string;
}

export interface ReplyTo {
  path: string[];
  ws_endpoint?: string;
}

export interface Hops {
  count: number;
  max: number;
  trace: string[];
}

export interface Envelope {
  msg_id: string;
  corr_id?: string;
  origin?: AgentAddr;
  sender: AgentAddr;
  recipient: AgentAddr;
  reply_to?: ReplyTo;
  intent: string;
  content_type?: string;
  priority?: string;
  deadline?: string;
  error?: string;
  hops?: Hops;
}

export interface PendingMessage {
  envelope?: Envelope;
  payload?: Record<string, unknown>;
  msg_id?: string;
  corr_id?: string;
  sender_agent?: string;
  recipient_agent?: string;
  intent?: string;
  status?: string;
  error?: string;
  created_at?: string | number;
  updated_at?: string | number;
  received_at?: string | number;
}

// Dashboard Types

export type DispatchStatus =
  | 'draft'
  | 'dispatched'
  | 'acknowledged'
  | 'accepted'
  | 'in_progress'
  | 'verification'
  | 'complete'
  | 'blocked'
  | 'delivery_failed'
  | 'contract_drift'
  | 'evidence_missing';

export type Intent =
  | 'delegate'
  | 'reply'
  | 'ack'
  | 'error'
  | 'stream_start'
  | 'stream_chunk'
  | 'stream_end';

export type EvidenceStatus = 'verified' | 'partial' | 'missing' | 'stale';
export type ContractStatus = 'match' | 'drift' | 'missing' | 'not_checked';
export type ApprovalState = 'required' | 'approved' | 'declined' | 'pending';
export type Risk = 'low' | 'medium' | 'high';

export interface MessageAttachment {
  name: string;
  type: string;
  size: number;
  data: string;
}

export interface Dispatch {
  dispatch_id: string;
  correlation_id: string;
  cdf_story_id?: string;
  intent: Intent;
  from: {
    agent_id: string;
    role?: string;
    machine_id: string;
  };
  to: {
    agent_id: string;
    role?: string;
    machine_id: string;
  };
  status: DispatchStatus;
  evidence_status: EvidenceStatus;
  contract_status: ContractStatus;
  approval_state: ApprovalState;
  required_approvals: string[];
  received_approvals: string[];
  last_updated: string;
  risk: Risk;
  payload_preview?: string;
  payload_content?: string;
  attachment?: MessageAttachment;
  deadline?: string;
  error?: string;
}

export interface Peer {
  agent_id: string;
  machine_id: string;
  http_endpoint: string;
  ws_endpoint?: string;
  capabilities?: string[];
  last_seen_at?: number;
}

export interface AgentHealth {
  peer: Peer;
  status: 'online' | 'offline' | 'degraded';
  queue_depth: number;
  retry_count: number;
  is_relay: boolean;
  active_work?: string;
  cdf_context_freshness?: 'fresh' | 'stale' | 'unknown';
  version_compatible?: boolean;
}

export interface HandoffContext {
  dispatch_id: string;
  correlation_id: string;
  objective?: {
    outcome?: string;
    stop_condition?: string;
  };
  active_work?: {
    story?: string;
    owner?: string;
    status?: string;
    acceptance_criteria_count?: number;
  };
  authority?: {
    canonical_sources: string[];
    approvals: { role: string; status: 'approved' | 'pending' | 'declined' }[];
  };
  constraints?: string[];
  evidence?: {
    last_command?: string;
    result?: string;
    environment?: string;
    known_gaps?: string[];
  };
  change_state?: {
    changed?: string[];
    unresolved?: string[];
    rollback?: string;
  };
  delivery_metadata?: {
    created_at: string;
    updated_at: string;
    last_transition?: string;
    reason?: string;
    next_action?: string;
    sla_deadline?: string;
  };
}

// Command Center metrics
export interface CommandCenterMetrics {
  active_stories: number;
  pending_handoffs: number;
  blocked_work: number;
  awaiting_approvals: number;
  contract_drift: number;
  unverified_evidence: number;
  offline_agents: number;
  recent_completions: number;
}

// Filters
export interface DispatchFilters {
  story?: string;
  cdf_phase?: string;
  role?: string;
  agent?: string;
  machine?: string;
  intent?: Intent;
  status?: DispatchStatus;
  approval_state?: ApprovalState;
  evidence_status?: EvidenceStatus;
  contract_drift?: boolean;
}

// Contract verification
export interface ContractVerificationResult {
  id: string;
  status: 'match' | 'drift' | 'missing' | 'undocumented';
  declared_contract?: {
    file: string;
    line?: number;
    content?: string;
  };
  actual_implementation?: {
    file: string;
    line?: number;
    content?: string;
  };
  owning_role?: string;
  recommended_action?: string;
}

export interface ContractVerificationSummary {
  match: number;
  drift: number;
  missing: number;
  undocumented: number;
  results: ContractVerificationResult[];
}

// Utility type for extracting role from agent_id
export function extractRole(agentId: string): string | undefined {
  const parts = agentId.split('-');
  if (parts.length >= 2) {
    const potentialRole = parts[0].toUpperCase();
    if (['FE', 'BE', 'QA', 'SA', 'PM', 'DEV', 'OPS'].includes(potentialRole)) {
      return potentialRole;
    }
  }
  return undefined;
}

// Status display configuration
export const STATUS_CONFIG: Record<DispatchStatus, { label: string; badge: string; icon: string }> = {
  draft: { label: 'Draft', badge: 'badge-offline', icon: 'FileText' },
  dispatched: { label: 'Dispatched', badge: 'badge-pending', icon: 'Send' },
  acknowledged: { label: 'Acknowledged', badge: 'badge-info', icon: 'Check' },
  accepted: { label: 'Accepted', badge: 'badge-info', icon: 'CheckCircle' },
  in_progress: { label: 'In Progress', badge: 'badge-pending', icon: 'Play' },
  verification: { label: 'Verification', badge: 'badge-warning', icon: 'Shield' },
  complete: { label: 'Complete', badge: 'badge-success', icon: 'CheckCircle2' },
  blocked: { label: 'Blocked', badge: 'badge-blocked', icon: 'AlertOctagon' },
  delivery_failed: { label: 'Delivery Failed', badge: 'badge-error', icon: 'XCircle' },
  contract_drift: { label: 'Contract Drift', badge: 'badge-error', icon: 'AlertTriangle' },
  evidence_missing: { label: 'Evidence Missing', badge: 'badge-warning', icon: 'FileX' },
};

export const INTENT_CONFIG: Record<Intent, { label: string; badge: string }> = {
  delegate: { label: 'Delegate', badge: 'badge-info' },
  reply: { label: 'Reply', badge: 'badge-success' },
  ack: { label: 'Ack', badge: 'badge-pending' },
  error: { label: 'Error', badge: 'badge-error' },
  stream_start: { label: 'Stream Start', badge: 'badge-warning' },
  stream_chunk: { label: 'Stream', badge: 'badge-warning' },
  stream_end: { label: 'Stream End', badge: 'badge-success' },
};
