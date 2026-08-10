import { useEffect, useMemo, useRef, useState } from 'react';
import { BrowserRouter, Link, Route, Routes, useLocation } from 'react-router-dom';
import {
  AlertCircle,
  ArrowRight,
  Check,
  ChevronRight,
  Circle,
  Clipboard,
  Download,
  Globe,
  Hash,
  Inbox as InboxIcon,
  MessageSquare,
  RefreshCw,
  Search,
  Send,
  Server,
  Settings2,
  ShieldCheck,
  SlidersHorizontal,
  Volume2,
  VolumeX,
  X,
  Zap,
} from 'lucide-react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {
  calculateMetrics,
  getAllMessages,
  getHealth,
  getPeers,
  sendMessage,
  transformToAgentHealth,
  transformToDispatch,
} from './api';
import type { AgentHealth, CommandCenterMetrics, Dispatch } from './types';
import { Button } from './components/ui/button';

const POLL_INTERVAL = 10_000;

type RelayStatus = { status: string; agent: string; this_agent_id?: string; this_machine_id?: string };

// ─── Data Hook ────────────────────────────────────────────────────────────────

function useData() {
  const [dispatches, setDispatches] = useState<Dispatch[]>([]);
  const [agents, setAgents] = useState<AgentHealth[]>([]);
  const [metrics, setMetrics] = useState<CommandCenterMetrics | null>(null);
  const [relay, setRelay] = useState<RelayStatus>({ status: 'connecting', agent: '' });
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);

  const load = async () => {
    try {
      const [health, messages, peers] = await Promise.all([
        getHealth(),
        getAllMessages(),
        getPeers(),
      ]);
      const nextDispatches = messages.messages.map(transformToDispatch);
      const nextAgents = transformToAgentHealth(peers.peers);
      setRelay(health);
      setDispatches(nextDispatches);
      setAgents(nextAgents);
      setMetrics(calculateMetrics(nextDispatches, nextAgents));
      setLastUpdated(new Date());
      setError(null);
    } catch (value) {
      setError(value instanceof Error ? value.message : 'Unable to load relay data');
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
    const interval = setInterval(() => void load(), POLL_INTERVAL);
    return () => clearInterval(interval);
  }, []);

  return { dispatches, agents, metrics, relay, error, loading, lastUpdated, load };
}

// ─── Shared UI Primitives ────────────────────────────────────────────────────

const STATUS_CONFIG: Record<string, { label: string; color: string }> = {
  online: { label: 'Online', color: 'teal' },
  offline: { label: 'Offline', color: 'slate' },
  degraded: { label: 'Degraded', color: 'amber' },
  complete: { label: 'Complete', color: 'teal' },
  approved: { label: 'Approved', color: 'teal' },
  acknowledged: { label: 'Ack', color: 'blue' },
  accepted: { label: 'Accepted', color: 'blue' },
  in_progress: { label: 'Running', color: 'blue' },
  verification: { label: 'Verify', color: 'amber' },
  blocked: { label: 'Blocked', color: 'coral' },
  delivery_failed: { label: 'Failed', color: 'coral' },
  contract_drift: { label: 'Drift', color: 'coral' },
  evidence_missing: { label: 'No Evidence', color: 'amber' },
  draft: { label: 'Draft', color: 'slate' },
  dispatched: { label: 'Sent', color: 'blue' },
  required: { label: 'Awaiting', color: 'amber' },
  pending: { label: 'Pending', color: 'slate' },
  declined: { label: 'Declined', color: 'coral' },
};

function StatusBadge({ value }: { value: string }) {
  const cfg = STATUS_CONFIG[value] ?? { label: value.replaceAll('_', ' '), color: 'slate' };
  return <span className={`badge badge--${cfg.color}`}>{cfg.label}</span>;
}

function StatusDot({ status }: { status: string }) {
  const color = status === 'online' ? 'teal' : status === 'offline' ? 'coral' : 'amber';
  return <span className={`status-dot status-dot--${color}`} />;
}

function AgentAvatar({ agentId, status }: { agentId: string; status: string }) {
  const initial = agentId.slice(0, 1).toUpperCase();
  const colorClass = status === 'offline' ? 'avatar--offline' : 'avatar--online';
  return <span className={`agent-avatar ${colorClass}`}>{initial}</span>;
}

// ─── Shell & Navigation ───────────────────────────────────────────────────────

function Shell({ relay, soundEnabled, onToggleSound, children }: { relay: RelayStatus; soundEnabled: boolean; onToggleSound: () => void; children: React.ReactNode }) {
  const location = useLocation();
  const online = relay.status === 'ok';
  const isAgent = !!relay.this_agent_id;
  const navItems = [
    { to: '/', icon: <SlidersHorizontal size={16} />, label: 'Dashboard' },
    { to: '/inbox', icon: <InboxIcon size={16} />, label: 'Inbox', live: online },
    { to: '/agents', icon: <Server size={16} />, label: 'Agents' },
    { to: '/setup', icon: <Settings2 size={16} />, label: 'Setup' },
  ];
  return (
    <div className="app-shell">
      <div className="bg-grid" aria-hidden />
      <header className="topbar">
        <Link className="brand" to="/">
          <span className="brand-mark">
            <img src="/acp-icon.svg" alt="" />
          </span>
          <span className="brand-name">ACP</span>
          <span className="brand-divider" />
          <span className="brand-sub">Control Plane</span>
        </Link>
        <div className="topbar-right">
          <div className="relay-pill">
            <StatusDot status={online ? 'online' : 'offline'} />
            <span>{online
              ? isAgent
                ? `Agent: ${relay.this_agent_id}`
                : 'Relay live'
              : relay.status}</span>
          </div>
          <Button variant="icon" className="icon-btn sound-btn" title={soundEnabled ? 'Mute message notifications' : 'Enable message notifications'} onClick={onToggleSound}>
            {soundEnabled ? <Volume2 size={15} /> : <VolumeX size={15} />}
             <span>{soundEnabled ? 'Sound on' : 'Sound off'}</span>
           </Button>
           <Button variant="icon" className="icon-btn" title="Refresh data">
             <RefreshCw size={15} />
           </Button>
        </div>
      </header>
      <aside className="sidebar">
        <nav className="primary-nav">
          {navItems.map((item) => (
            <Link
              key={item.to}
              className={`nav-item ${location.pathname === item.to ? 'nav-item--active' : ''}`}
              to={item.to}
            >
              {item.icon}
              <span>{item.label}</span>
              {item.live && <span className="live-dot" />}
            </Link>
          ))}
        </nav>
        <div className="sidebar-footer">
          <div className="relay-card">
            <StatusDot status={online ? 'online' : 'offline'} />
            <div className="relay-card-text">
              <strong>{isAgent ? relay.this_agent_id : relay.agent || 'Relay'}</strong>
              <small>{online ? `${isAgent ? 'Agent' : 'Relay'} · v1.0` : 'Disconnected'}</small>
            </div>
          </div>
          <p className="sidebar-note">Auto-refreshes every 10s · Click any metric to explore</p>
        </div>
      </aside>
      <main className="main-content">{children}</main>
    </div>
  );
}

// ─── Page Header ─────────────────────────────────────────────────────────────

function PageHeader({
  eyebrow,
  title,
  description,
  actions,
}: {
  eyebrow: string;
  title: string;
  description: string;
  actions?: React.ReactNode;
}) {
  return (
    <div className="page-header">
      <div className="page-header-text">
        <p className="eyebrow">{eyebrow}</p>
        <h1>{title}</h1>
        <p className="page-description">{description}</p>
      </div>
      {actions && <div className="page-header-actions">{actions}</div>}
    </div>
  );
}

// ─── Metric Card ──────────────────────────────────────────────────────────────

function MetricCard({
  label,
  value,
  accent,
  onClick,
  active,
}: {
  label: string;
  value: number;
  accent?: string;
  onClick?: () => void;
  active?: boolean;
}) {
  return (
    <button
      className={`metric-card metric-card--${accent ?? 'default'} ${onClick ? 'metric-card--clickable' : ''} ${active ? 'metric-card--active' : ''}`}
      onClick={onClick}
    >
      <span className="metric-card-label">{label}</span>
      <strong className="metric-card-value">{value}</strong>
      {onClick && <ChevronRight size={13} className="metric-card-arrow" />}
    </button>
  );
}

// ─── Dashboard / Command Center ───────────────────────────────────────────────

function Dashboard({ data }: { data: ReturnType<typeof useData> }) {
  const { metrics, agents, dispatches, relay, loading, error, load } = data;
  const recent = dispatches.slice(-8).reverse();
  const alertMetrics = metrics
    ? [
        { label: 'Blocked work', value: metrics.blocked_work, accent: 'coral' },
        { label: 'Contract drift', value: metrics.contract_drift, accent: 'coral' },
        { label: 'Offline agents', value: metrics.offline_agents, accent: 'violet' },
        { label: 'Unverified evidence', value: metrics.unverified_evidence, accent: 'amber' },
      ].filter((m) => m.value > 0)
    : [];

  return (
    <div className="page">
      <PageHeader
        eyebrow="Overview"
        title="Mission Control"
        description="Real-time view of your agent network, message flow, and system health."
        actions={
          <Button onClick={() => void load()}>
            <RefreshCw size={14} />
            Refresh
          </Button>
        }
      />
      {error && (
        <div className="alert alert--error">
          <AlertCircle size={16} />
          {error}
        </div>
      )}
      {loading && !metrics ? (
        <div className="loading-state">
          <div className="spinner" />
          <span>Connecting to relay…</span>
        </div>
      ) : (
        <>
          {/* System health banner */}
          <section className="health-banner">
            <div className={`health-banner-icon ${relay.status === 'ok' ? 'health-banner-icon--ok' : 'health-banner-icon--warn'}`}>
              {relay.status === 'ok' ? <Zap size={20} /> : <AlertCircle size={20} />}
            </div>
            <div className="health-banner-body">
              <p className="eyebrow">System</p>
              <h2>{relay.status === 'ok' ? 'All systems operational' : relay.this_agent_id ? 'Agent needs attention' : 'Relay needs attention'}</h2>
              <p>{relay.this_agent_id ? `${relay.this_agent_id}@${relay.this_machine_id || 'unknown'}` : relay.agent || 'No identity'} · ACP v1.0</p>
            </div>
            <StatusBadge value={relay.status} />
          </section>

          {/* Metric strip */}
          <section className="metric-strip">
            <MetricCard label="Total messages" value={dispatches.length} />
            <MetricCard label="In progress" value={metrics?.active_stories ?? 0} accent="blue" />
            <MetricCard label="Pending handoffs" value={metrics?.pending_handoffs ?? 0} accent="teal" />
            <MetricCard label="Completed today" value={metrics?.recent_completions ?? 0} accent="teal" />
            {alertMetrics.map((m) => (
              <MetricCard key={m.label} label={m.label} value={m.value} accent={m.accent} />
            ))}
          </section>

          {/* Main grid */}
          <div className="dashboard-grid">
            {/* Recent messages */}
            <section className="panel">
              <div className="panel-header">
                <div>
                  <p className="eyebrow">Traffic</p>
                  <h2>Recent messages</h2>
                </div>
                <Link className="btn btn--ghost btn--sm" to="/inbox">
                  View all <ArrowRight size={13} />
                </Link>
              </div>
              {recent.length ? (
                <MessageFeed dispatches={recent} compact />
              ) : (
                <div className="empty-state">
                  <MessageSquare size={28} />
                  <p>No messages yet</p>
                </div>
              )}
            </section>

            {/* Agents */}
            <section className="panel">
              <div className="panel-header">
                <div>
                  <p className="eyebrow">Network</p>
                  <h2>Agents ({agents.length})</h2>
                </div>
                <Link className="btn btn--ghost btn--sm" to="/agents">
                  Manage <ArrowRight size={13} />
                </Link>
              </div>
              {agents.length ? (
                <AgentFeed agents={agents} />
              ) : (
                <div className="empty-state">
                  <Server size={28} />
                  <p>No agents registered</p>
                </div>
              )}
            </section>
          </div>
        </>
      )}
    </div>
  );
}

// ─── Message Feed (compact list) ──────────────────────────────────────────────

function MessageFeed({ dispatches, compact }: { dispatches: Dispatch[]; compact?: boolean }) {
  return (
    <div className={`message-feed ${compact ? 'message-feed--compact' : ''}`}>
      {dispatches.map((d) => (
        <div key={d.dispatch_id} className="message-feed-row">
          <span className={`intent-chip intent-chip--${d.intent}`}>{d.intent}</span>
          <div className="message-feed-route">
            <span className="agent-tag">{d.from.agent_id}</span>
            <ArrowRight size={12} className="route-arrow" />
            <span className="agent-tag">{d.to.agent_id}</span>
          </div>
          <StatusBadge value={d.status} />
        </div>
      ))}
    </div>
  );
}

// ─── Agent Feed ────────────────────────────────────────────────────────────────

function AgentFeed({ agents }: { agents: AgentHealth[] }) {
  return (
    <div className="agent-feed">
      {agents.map((a) => (
        <div key={a.peer.agent_id} className="agent-feed-row">
          <AgentAvatar agentId={a.peer.agent_id} status={a.status} />
          <div className="agent-feed-info">
            <strong>{a.peer.agent_id}</strong>
            <small>{a.peer.machine_id}</small>
          </div>
          <div className="agent-feed-meta">
            <StatusBadge value={a.status} />
            {a.queue_depth > 0 && <span className="queue-badge">{a.queue_depth} queued</span>}
          </div>
        </div>
      ))}
    </div>
  );
}

// ─── Inbox ───────────────────────────────────────────────────────────────────

type FilterStatus = 'all' | Dispatch['status'];
const STATUS_FILTERS: FilterStatus[] = ['all', 'in_progress', 'complete', 'blocked', 'dispatched', 'verification'];

function Inbox({ data }: { data: ReturnType<typeof useData> }) {
  const [query, setQuery] = useState('');
  const [statusFilter, setStatusFilter] = useState<FilterStatus>('all');
  const [selected, setSelected] = useState<Dispatch | null>(null);
  const [composeOpen, setComposeOpen] = useState(false);
  const [copiedId, setCopiedId] = useState(false);

  const filtered = useMemo(() => {
    return data.dispatches.filter((item) => {
      const matchesQuery = query === '' ||
        `${item.from.agent_id} ${item.to.agent_id} ${item.intent} ${item.payload_preview ?? ''} ${item.status}`
          .toLowerCase()
          .includes(query.toLowerCase());
      const matchesStatus = statusFilter === 'all' || item.status === statusFilter;
      return matchesQuery && matchesStatus;
    });
  }, [data.dispatches, query, statusFilter]);

  useEffect(() => {
    if (!selected) return;
    const close = (e: KeyboardEvent) => { if (e.key === 'Escape') setSelected(null); };
    document.addEventListener('keydown', close);
    return () => document.removeEventListener('keydown', close);
  }, [selected]);

  const copyId = async () => {
    if (!selected) return;
    await navigator.clipboard.writeText(selected.dispatch_id);
    setCopiedId(true);
    window.setTimeout(() => setCopiedId(false), 1800);
  };

  return (
    <div className="page">
      <PageHeader
        eyebrow="Messages"
        title="Inbox"
        description="Messages involving this agent — searchable and filterable."
        actions={
          <Button variant="primary" onClick={() => setComposeOpen(true)}>
            <Send size={14} /> New message
          </Button>
        }
      />
      <div className="inbox-layout">
        {/* Message list */}
        <div className="inbox-list-panel panel">
          <div className="inbox-toolbar">
            <div className="search-box">
              <Search size={15} />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search agents, intents, content…"
                className="search-input"
              />
            </div>
          </div>
          <div className="filter-chips">
            {STATUS_FILTERS.map((s) => (
              <button
                key={s}
                className={`filter-chip ${statusFilter === s ? 'filter-chip--active' : ''}`}
                onClick={() => setStatusFilter(s)}
              >
                {s === 'all' ? 'All' : STATUS_CONFIG[s]?.label ?? s}
              </button>
            ))}
          </div>
          <div className="message-list">
            {data.loading ? (
              <div className="loading-state">
                <div className="spinner" />
              </div>
            ) : filtered.length === 0 ? (
              <div className="empty-state">
                <Search size={24} />
                <p>No messages match your filters</p>
              </div>
            ) : (
              filtered.map((d) => (
                <button
                  key={d.dispatch_id}
                  className={`message-row ${selected?.dispatch_id === d.dispatch_id ? 'message-row--selected' : ''}`}
                  onClick={() => setSelected(d)}
                >
                  <div className="message-row-left">
                    <span className={`intent-chip intent-chip--${d.intent}`}>{d.intent}</span>
                    <div className="message-row-main">
                      <span className="message-row-route">
                        <span className="agent-tag">{d.from.agent_id}</span>
                        <ArrowRight size={11} />
                        <span className="agent-tag">{d.to.agent_id}</span>
                      </span>
                      <span className="message-row-preview">
                        {d.payload_preview ?? `${d.intent} message`}
                      </span>
                    </div>
                  </div>
                  <div className="message-row-right">
                    <StatusBadge value={d.status} />
                    <span className="message-id">{d.dispatch_id.slice(0, 8)}</span>
                  </div>
                </button>
              ))
            )}
          </div>
        </div>

        {/* Detail panel */}
        <div
          className={selected ? 'modal-backdrop inbox-detail-modal' : 'inbox-detail-panel'}
          onMouseDown={(e) => { if (selected && e.target === e.currentTarget) setSelected(null); }}
        >
          {selected ? (
            <div className="panel detail-panel">
              <div className="detail-header">
                <div>
                  <p className="eyebrow">Message detail</p>
                  <h2>{selected.intent} message</h2>
                </div>
                <button className="icon-btn icon-btn--ghost" onClick={() => setSelected(null)}>
                  <X size={16} />
                </button>
              </div>
              <div className="detail-meta">
                <StatusBadge value={selected.status} />
                <code className="detail-id">{selected.dispatch_id}</code>
                <button className="btn btn--ghost btn--sm" onClick={copyId}>
                  {copiedId ? <Check size={12} /> : <Clipboard size={12} />}
                  {copiedId ? 'Copied' : 'Copy ID'}
                </button>
              </div>
              <div className="detail-routes">
                <div className="detail-route-item">
                  <span className="detail-route-label">From</span>
                  <span className="agent-tag">{selected.from.agent_id}</span>
                  <small>{selected.from.machine_id}</small>
                </div>
                <ArrowRight size={16} className="detail-route-arrow" />
                <div className="detail-route-item">
                  <span className="detail-route-label">To</span>
                  <span className="agent-tag">{selected.to.agent_id}</span>
                  <small>{selected.to.machine_id}</small>
                </div>
              </div>
              <div className="detail-fields">
                <div className="detail-field">
                  <span className="detail-field-label">Correlation ID</span>
                  <code>{selected.correlation_id}</code>
                </div>
                <div className="detail-field">
                  <span className="detail-field-label">Intent</span>
                  <StatusBadge value={selected.intent} />
                </div>
                <div className="detail-field">
                  <span className="detail-field-label">Risk</span>
                  <span className={`risk-chip risk-chip--${selected.risk}`}>{selected.risk}</span>
                </div>
                <div className="detail-field">
                  <span className="detail-field-label">Approval</span>
                  <StatusBadge value={selected.approval_state} />
                </div>
                <div className="detail-field">
                  <span className="detail-field-label">Evidence</span>
                  <StatusBadge value={selected.evidence_status} />
                </div>
                <div className="detail-field">
                  <span className="detail-field-label">Contract</span>
                  <StatusBadge value={selected.contract_status} />
                </div>
              </div>
              {selected.payload_content && (
                <div className="detail-payload">
                  <p className="eyebrow">Payload</p>
                  <div className="markdown-body payload-body">
                    <Markdown remarkPlugins={[remarkGfm]}>{selected.payload_content}</Markdown>
                  </div>
                </div>
              )}
            </div>
          ) : (
            <div className="panel empty-detail-panel">
              <div className="empty-state">
                <MessageSquare size={36} />
                <h3>Select a message</h3>
                <p>Click any message on the left to see its full details here.</p>
              </div>
            </div>
          )}
        </div>
      </div>

      {composeOpen && (
        <ComposeModal
          onClose={() => setComposeOpen(false)}
          onSent={() => { setComposeOpen(false); void data.load(); }}
          agents={data.agents}
          thisAgentId={data.relay.this_agent_id || 'naiplawan-agent'}
          thisMachineId={data.relay.this_machine_id || 'naiplawan-machine'}
        />
      )}
    </div>
  );
}

// ─── Agents Page ─────────────────────────────────────────────────────────────

function Agents({ data }: { data: ReturnType<typeof useData> }) {
  const { agents, loading } = data;
  const online = agents.filter((a) => a.status === 'online');
  const offline = agents.filter((a) => a.status === 'offline');

  return (
    <div className="page">
      <PageHeader
        eyebrow="Network"
        title="Agents"
        description={`${agents.length} registered agent${agents.length !== 1 ? 's' : ''} — ${online.length} online, ${offline.length} offline.`}
        actions={
          <button className="btn btn--ghost" onClick={() => void data.load()}>
            <RefreshCw size={14} /> Refresh
          </button>
        }
      />
      {loading && !agents.length ? (
        <div className="loading-state">
          <div className="spinner" />
          <span>Loading agents…</span>
        </div>
      ) : agents.length === 0 ? (
        <div className="panel">
          <div className="empty-state">
            <Server size={36} />
            <h3>No agents found</h3>
            <p>Agents will appear here once they connect to the relay.</p>
          </div>
        </div>
      ) : (
        <div className="agents-grid">
          {agents.map((a) => (
            <div key={a.peer.agent_id} className={`agent-card ${a.status === 'offline' ? 'agent-card--offline' : ''}`}>
              <div className="agent-card-header">
                <AgentAvatar agentId={a.peer.agent_id} status={a.status} />
                <div className="agent-card-info">
                  <strong>{a.peer.agent_id}</strong>
                  <small>{a.peer.machine_id}</small>
                </div>
                <StatusBadge value={a.status} />
              </div>
              <div className="agent-card-details">
                <div className="agent-card-detail">
                  <Globe size={13} />
                  <code>{a.peer.http_endpoint}</code>
                </div>
                {a.peer.capabilities && a.peer.capabilities.length > 0 && (
                  <div className="agent-card-capabilities">
                    {a.peer.capabilities.map((cap) => (
                      <span key={cap} className="capability-chip">{cap}</span>
                    ))}
                  </div>
                )}
                <div className="agent-card-stats">
                  <div className="agent-stat">
                    <span className="agent-stat-value">{a.queue_depth}</span>
                    <span className="agent-stat-label">Queue</span>
                  </div>
                  <div className="agent-stat">
                    <span className="agent-stat-value">{a.retry_count}</span>
                    <span className="agent-stat-label">Retries</span>
                  </div>
                  <div className="agent-stat">
                    <StatusDot status={a.status} />
                    <span className="agent-stat-label">{a.status}</span>
                  </div>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ─── Setup Page ───────────────────────────────────────────────────────────────

type SetupConfig = {
  relayUrl: string;
  agentId: string;
  machineId: string;
  endpoint: string;
  capabilities: string;
  pollInterval: string;
};

const defaultSetup: SetupConfig = {
  relayUrl: 'http://localhost:8444',
  agentId: 'my-agent',
  machineId: 'my-machine',
  endpoint: 'http://localhost:8444',
  capabilities: 'agent, signaling',
  pollInterval: '3',
};

function setupYaml(config: SetupConfig) {
  const capabilities = config.capabilities.split(',').map((item) => item.trim()).filter(Boolean);
  return `config_version: 1

this_agent:
  agent_id: ${config.agentId}
  machine_id: ${config.machineId}
  http_endpoint: ${config.endpoint}
  capabilities:
${capabilities.map((item) => `    - ${item}`).join('\n')}

peers:
  - agent_id: acp-relay
    machine_id: relay
    http_endpoint: ${config.relayUrl}
    capabilities:
      - relay
      - registry

security:
  default_auth_type: signed-token
  token_ttl_seconds: 3600
  require_https: false
`;
}

function Setup() {
  const [config, setConfig] = useState<SetupConfig>(() => {
    try {
      return { ...defaultSetup, ...JSON.parse(localStorage.getItem('acp-setup-config') || '{}') };
    } catch {
      return defaultSetup;
    }
  });
  const [copied, setCopied] = useState(false);
  const [saved, setSaved] = useState(false);
  const yaml = setupYaml(config);
  const update = (key: keyof SetupConfig, value: string) =>
    setConfig((current) => ({ ...current, [key]: value }));
  const save = () => {
    localStorage.setItem('acp-setup-config', JSON.stringify(config));
    setSaved(true);
    window.setTimeout(() => setSaved(false), 2000);
  };
  const copy = async () => {
    await navigator.clipboard.writeText(yaml);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1800);
  };
  const download = () => {
    const link = document.createElement('a');
    link.href = URL.createObjectURL(new Blob([yaml], { type: 'text/yaml' }));
    link.download = 'acp-peers.yaml';
    link.click();
    URL.revokeObjectURL(link.href);
  };

  return (
    <div className="page">
      <PageHeader
        eyebrow="Workspace"
        title="Setup"
        description="Configure your agent identity and relay connection — no protocol code required."
      />
      <div className="setup-layout">
        {/* Form */}
        <div className="panel setup-form">
          <div className="panel-header">
            <div>
              <p className="eyebrow">Step 1 — Identity</p>
              <h2>Agent details</h2>
            </div>
            <span className="step-num">01</span>
          </div>
          <div className="form-body">
            <div className="form-row">
              <label className="form-label">
                Agent ID
                <input
                  className="form-input"
                  value={config.agentId}
                  onChange={(e) => update('agentId', e.target.value)}
                  placeholder="my-agent"
                />
              </label>
              <label className="form-label">
                Machine ID
                <input
                  className="form-input"
                  value={config.machineId}
                  onChange={(e) => update('machineId', e.target.value)}
                  placeholder="my-machine"
                />
              </label>
            </div>
            <label className="form-label form-label--full">
              HTTP Endpoint
              <input
                className="form-input"
                value={config.endpoint}
                onChange={(e) => update('endpoint', e.target.value)}
                placeholder="http://localhost:8444"
              />
            </label>
            <label className="form-label form-label--full">
              Capabilities <span className="form-hint">comma-separated</span>
              <input
                className="form-input"
                value={config.capabilities}
                onChange={(e) => update('capabilities', e.target.value)}
                placeholder="agent, signaling"
              />
            </label>
          </div>

          <div className="panel-header panel-header--step">
            <div>
              <p className="eyebrow">Step 2 — Server</p>
              <h2>Connection</h2>
            </div>
            <span className="step-num">02</span>
          </div>
          <div className="form-body">
            <label className="form-label form-label--full">
              Server URL <span className="form-hint">Agent or Relay endpoint</span>
              <input
                className="form-input"
                value={config.relayUrl}
                onChange={(e) => update('relayUrl', e.target.value)}
                placeholder="http://localhost:8443"
              />
            </label>
            <label className="form-label form-label--half">
              Poll interval <span className="form-hint">seconds</span>
              <input
                className="form-input"
                value={config.pollInterval}
                onChange={(e) => update('pollInterval', e.target.value)}
                inputMode="numeric"
              />
            </label>
          </div>

          <div className="security-note">
            <ShieldCheck size={16} />
            <div>
              <strong>Server-side secrets</strong>
              <p>
                Set <code>ACP_SHARED_SECRET</code> and <code>OPENROUTER_API_KEY</code> in{' '}
                <code>acp-server/.env.local</code>. They are never stored in this browser.
              </p>
            </div>
          </div>

          <div className="form-footer">
            <Button variant="primary" onClick={save}>
              {saved ? <Check size={14} /> : <Download size={14} />}
              {saved ? 'Saved!' : 'Save on this device'}
            </Button>
          </div>
        </div>

        {/* YAML preview */}
        <div className="panel yaml-panel">
          <div className="panel-header">
            <div>
              <p className="eyebrow">Step 3 — Output</p>
              <h2>Peer config</h2>
            </div>
            <button className="btn btn--ghost btn--sm" onClick={copy}>
              {copied ? <Check size={13} /> : <Clipboard size={13} />}
              {copied ? 'Copied' : 'Copy'}
            </button>
          </div>
          <pre className="yaml-pre"><code>{yaml}</code></pre>
          <div className="yaml-actions">
            <button className="btn btn--outline" onClick={download}>
              <Download size={13} /> Download acp-peers.yaml
            </button>
            <p className="yaml-note">
              Use with <code>ACP_PEERS_PATH</code> or place at <code>~/.acp/acp-peers.yaml</code>
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}

// ─── Compose Modal ────────────────────────────────────────────────────────────

function ComposeModal({
  onClose,
  onSent,
  agents,
  thisAgentId,
  thisMachineId,
}: {
  onClose: () => void;
  onSent: () => void;
  agents: AgentHealth[];
  thisAgentId: string;
  thisMachineId: string;
}) {
  const [recipient, setRecipient] = useState('naiplawan-agent');
  const [message, setMessage] = useState('');
  const [link, setLink] = useState('');
  const [attachment, setAttachment] = useState<{ name: string; type: string; size: number; data: string } | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selectFile = (file: File | undefined) => {
    if (!file) return;
    if (file.size > 5 * 1024 * 1024) {
      setError('Files must be 5 MB or smaller.');
      return;
    }
    const reader = new FileReader();
    reader.onload = () =>
      setAttachment({ name: file.name, type: file.type || 'application/octet-stream', size: file.size, data: String(reader.result) });
    reader.readAsDataURL(file);
  };

  const submit = async () => {
    if (!recipient.trim() || (!message.trim() && !link.trim() && !attachment)) {
      setError('Add a recipient and at least a message, link, or file.');
      return;
    }
    setSending(true);
    setError(null);
    const msgId = `msg_${crypto.randomUUID().replaceAll('-', '').slice(0, 12)}`;
    const sender = { agent_id: thisAgentId, machine_id: thisMachineId };
    const envelope = {
      msg_id: msgId,
      corr_id: msgId,
      origin: sender,
      sender,
      recipient: { agent_id: recipient.trim(), machine_id: thisMachineId },
      reply_to: [`${thisAgentId}@${thisMachineId}`],
      hops: { count: 0, max: 10, trace: [] },
      intent: 'delegate',
      content_type: 'application/json',
      priority: 'normal',
      deadline: null,
    };
    try {
      await sendMessage(envelope, {
        message: message.trim() || undefined,
        link: link.trim() || undefined,
        attachment: attachment
          ? { name: attachment.name, type: attachment.type, size: attachment.size, data: attachment.data }
          : undefined,
      });
      onSent();
    } catch (value) {
      setError(value instanceof Error ? value.message : 'Unable to send message');
    } finally {
      setSending(false);
    }
  };

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <section className="compose-modal" role="dialog" aria-modal="true" aria-labelledby="compose-title">
        <div className="modal-header">
          <div>
            <p className="eyebrow">New message</p>
            <h2 id="compose-title">Send to an agent</h2>
          </div>
           <button className="icon-btn icon-btn--ghost" onClick={onClose} aria-label="Close">
             <X size={18} />
           </button>
        </div>

        {agents.length > 0 && (
          <div className="compose-recipients">
            <p className="eyebrow">Known agents</p>
            <div className="recipient-chips">
              {agents.map((a) => (
                <button
                  key={a.peer.agent_id}
                  className={`recipient-chip ${a.status === 'offline' ? 'recipient-chip--offline' : ''}`}
                  onClick={() => setRecipient(a.peer.agent_id)}
                >
                  <Circle size={8} fill={a.status === 'online' ? 'var(--teal)' : 'var(--slate)'} />
                  {a.peer.agent_id}
                </button>
              ))}
            </div>
          </div>
        )}

        <div className="compose-form">
          <label className="form-label form-label--full">
            Recipient
            <input
              className="form-input"
              value={recipient}
              onChange={(e) => setRecipient(e.target.value)}
              placeholder="agent-id"
            />
          </label>
          <label className="form-label form-label--full">
            Message
            <textarea
              className="form-input form-textarea"
              value={message}
              onChange={(e) => setMessage(e.target.value)}
              placeholder="What should the agent do?"
              rows={5}
            />
          </label>
          <label className="form-label form-label--full">
            Link <span className="form-hint">optional</span>
            <input
              className="form-input"
              value={link}
              onChange={(e) => setLink(e.target.value)}
              placeholder="https://example.com/context"
              type="url"
            />
          </label>
          <label className="form-label form-label--full">
            File <span className="form-hint">optional · max 5 MB</span>
            <input type="file" className="form-input form-input--file" onChange={(e) => selectFile(e.target.files?.[0])} />
            {attachment && (
              <small className="file-attached">
                <Hash size={12} /> {attachment.name} · {(attachment.size / 1024).toFixed(0)} KB
              </small>
            )}
          </label>
        </div>

        {error && <div className="compose-error">{error}</div>}

        <div className="compose-footer">
          <button className="btn btn--ghost" onClick={onClose}>Cancel</button>
           <Button variant="primary" onClick={() => void submit()} disabled={sending}>
             <Send size={14} />
             {sending ? 'Sending…' : 'Send message'}
          </Button>
        </div>
      </section>
    </div>
  );
}

// ─── App ──────────────────────────────────────────────────────────────────────

function App() {
  const data = useData();
  const [soundEnabled, setSoundEnabled] = useState(() => localStorage.getItem('acp-sound-enabled') === 'true');
  const audioContext = useRef<AudioContext | null>(null);
  const seenMessages = useRef<Set<string> | null>(null);
  const playNotification = () => {
    const context = audioContext.current || new AudioContext();
    audioContext.current = context;
    const now = context.currentTime;
    [660, 880].forEach((frequency, index) => {
      const oscillator = context.createOscillator();
      const gain = context.createGain();
      oscillator.type = 'sine';
      oscillator.frequency.value = frequency;
      gain.gain.setValueAtTime(0.0001, now + index * 0.11);
      gain.gain.exponentialRampToValueAtTime(0.12, now + index * 0.11 + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + index * 0.11 + 0.16);
      oscillator.connect(gain).connect(context.destination);
      oscillator.start(now + index * 0.11);
      oscillator.stop(now + index * 0.11 + 0.17);
    });
  };
  useEffect(() => {
    if (data.loading) return;
    const currentIds = new Set(data.dispatches.map((message) => message.dispatch_id));
    if (seenMessages.current && soundEnabled && [...currentIds].some((id) => !seenMessages.current?.has(id))) playNotification();
    seenMessages.current = currentIds;
  }, [data.dispatches, data.loading, soundEnabled]);
  const toggleSound = () => {
    const next = !soundEnabled;
    setSoundEnabled(next);
    localStorage.setItem('acp-sound-enabled', String(next));
    if (next) playNotification();
  };
  return (
    <BrowserRouter>
      <Shell relay={data.relay} soundEnabled={soundEnabled} onToggleSound={toggleSound}>
        <Routes>
          <Route path="/" element={<Dashboard data={data} />} />
          <Route path="/inbox" element={<Inbox data={data} />} />
          <Route path="/agents" element={<Agents data={data} />} />
          <Route path="/setup" element={<Setup />} />
        </Routes>
      </Shell>
    </BrowserRouter>
  );
}

export default App;
