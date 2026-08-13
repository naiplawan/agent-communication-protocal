import { useEffect, useMemo, useRef, useState } from 'react';
import { BrowserRouter, Link, Route, Routes, useLocation, useSearchParams } from 'react-router-dom';
import {
  Activity,
  AlertCircle,
  ArrowRight,
  Bell,
  BellOff,
  Check,
  CheckCircle2,
  ChevronRight,
  Clock3,
  Circle,
  Clipboard,
  Copy,
  Cpu,
  Download,
  ExternalLink,
  Globe,
  Hash,
  Inbox as InboxIcon,
  MessageSquare,
  Radio,
  RefreshCw,
  Search,
  Send,
  Server,
  Settings2,
  ShieldCheck,
  SlidersHorizontal,
  Target,
  Volume2,
  VolumeX,
  Wifi,
  WifiOff,
  X,
  Zap,
} from 'lucide-react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {
  calculateMetrics,
  acknowledgeMessage,
  getAllMessages,
  getHealth,
  getPeers,
  sendMessage,
  transformToAgentHealth,
  transformToDispatch,
} from './api';
import type { AgentHealth, CommandCenterMetrics, Dispatch, Peer, PendingMessage } from './types';
import { Button } from './components/ui/button';

const POLL_INTERVAL = 10_000;
const WS_RECONNECT_DELAY = 3000;
const WS_OPEN_TIMEOUT = 3000;

type RelayStatus = { status: string; agent: string; this_agent_id?: string; this_machine_id?: string };

// ─── WebSocket Manager ────────────────────────────────────────────────────────
// The relay exposes a live WebSocket endpoint, with polling as the fallback
// when the endpoint is unavailable.

type WsMessage = {
  type: 'message' | 'ack' | 'error' | 'presence';
  data: unknown;
};

function useWebSocket(onMessage: (msg: WsMessage) => void) {
  const [connected, setConnected] = useState(false);
  const url = useMemo(() => {
    const base = (import.meta.env.VITE_RELAY_URL || '/api/relay')
      .replace(/^http/, 'ws')
      .replace(/\/acp\/v1.*$/, '');
    return `${base}/acp/stream/live`;
  }, []);

  // Callers pass a fresh closure every render. Holding it in a ref keeps the
  // latest handler reachable without making it a reconnect trigger.
  const onMessageRef = useRef(onMessage);
  useEffect(() => {
    onMessageRef.current = onMessage;
  });

  useEffect(() => {
    let socket: WebSocket | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let openTimer: ReturnType<typeof setTimeout> | null = null;
    let cancelled = false;

    const connect = () => {
      if (cancelled) return;
      try {
        const ws = new WebSocket(url);
        socket = ws;

        // A relay without a live endpoint never completes the upgrade, so give
        // up on silence and let onclose schedule the retry.
        openTimer = setTimeout(() => {
          if (ws.readyState !== WebSocket.OPEN) ws.close();
        }, WS_OPEN_TIMEOUT);

        ws.onopen = () => {
          if (openTimer) clearTimeout(openTimer);
          setConnected(true);
        };
        ws.onclose = () => {
          if (openTimer) clearTimeout(openTimer);
          setConnected(false);
          if (!cancelled) reconnectTimer = setTimeout(connect, WS_RECONNECT_DELAY);
        };
        ws.onerror = () => ws.close();
        ws.onmessage = (e) => {
          try {
            onMessageRef.current(JSON.parse(e.data) as WsMessage);
          } catch {
            /* ignore parse errors */
          }
        };
      } catch {
        setConnected(false);
      }
    };

    connect();

    return () => {
      cancelled = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      if (openTimer) clearTimeout(openTimer);
      socket?.close();
    };
  }, [url]);

  return { connected };
}

// ─── Desktop Notifications ─────────────────────────────────────────────────────

function useNotifications(enabled: boolean) {
  const requestPermission = () => {
    if (Notification.permission === 'default') {
      Notification.requestPermission();
    }
  };

  const notify = (title: string, body: string, icon?: string) => {
    if (!enabled || Notification.permission !== 'granted') return;
    try {
      new Notification(title, { body, icon: icon ?? '/acp-icon.svg', silent: true });
    } catch { /* quotas or blocked */ }
  };

  return { notify, requestPermission };
}

// ─── Data Hook ────────────────────────────────────────────────────────────────

function useData(opts: { onNewMessage?: (d: Dispatch) => void } = {}) {
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
      const relayAgentId = health.agent || 'acp-relay';
      const isRelayDashboard = !health.this_agent_id;
      const isRelayPeer = (peer: Peer) =>
        peer.agent_id === relayAgentId ||
        peer.agent_id === 'acp-relay' ||
        peer.machine_id === 'relay' ||
        peer.capabilities?.includes('relay') === true;

      // Merge registered peers + agents seen in message history
      const seenAgentIds = new Set<string>();
      const seenMachines = new Map<string, string>(); // agent_id -> machine_id
      for (const msg of messages.messages) {
        if (msg.sender_agent) seenAgentIds.add(msg.sender_agent);
        if (msg.recipient_agent) seenAgentIds.add(msg.recipient_agent);
      }
      for (const peer of peers.peers) {
        if (!isRelayDashboard || !isRelayPeer(peer)) {
          seenAgentIds.add(peer.agent_id);
          seenMachines.set(peer.agent_id, peer.machine_id);
        }
      }
      const visiblePeers = isRelayDashboard
        ? peers.peers.filter((peer) => !isRelayPeer(peer))
        : peers.peers;
      const mergedPeers: Peer[] = visiblePeers.concat(
        [...seenAgentIds]
          .filter((id) => !visiblePeers.some((p) => p.agent_id === id))
          .map((agent_id) => ({
            agent_id,
            machine_id: seenMachines.get(agent_id) || 'unknown',
            http_endpoint: '',
            capabilities: [],
          }))
      );

      const nextAgents = transformToAgentHealth(mergedPeers);
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

  // WebSocket real-time updates
  const handleWsMessage = (msg: WsMessage) => {
    if (msg.type === 'message' && msg.data && typeof msg.data === 'object') {
      const pm = msg.data as PendingMessage;
      if (pm.msg_id) {
        const dispatch = transformToDispatch(pm);
        setDispatches((prev) => {
          // Avoid duplicates, prepend new messages
          if (prev.some((d) => d.dispatch_id === dispatch.dispatch_id)) return prev;
          return [dispatch, ...prev];
        });
        setMetrics((prev) =>
          prev
            ? { ...prev, pending_handoffs: prev.pending_handoffs + 1 }
            : prev
        );
        opts.onNewMessage?.(dispatch);
      }
    }
    // Presence updates trigger a full reload
    if (msg.type === 'presence') {
      void load();
    }
  };

  const { connected } = useWebSocket(handleWsMessage);

  useEffect(() => {
    // `load` sets no state synchronously — every setState runs after the awaited
    // fetches resolve — so this is a subscription to an external system rather
    // than the cascading render the rule guards against.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void load();
    const interval = setInterval(() => void load(), POLL_INTERVAL);
    return () => clearInterval(interval);
  }, []);

  return {
    dispatches,
    agents,
    metrics,
    relay,
    error,
    loading,
    lastUpdated,
    load,
    wsConnected: connected,
  };
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

function formatRelativeTime(value?: Date | number) {
  if (!value) return 'Not available';
  const timestamp = value instanceof Date ? value.getTime() : value * 1000;
  const seconds = Math.max(0, Math.floor((Date.now() - timestamp) / 1000));
  if (seconds < 10) return 'just now';
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

// ─── Shell & Navigation ───────────────────────────────────────────────────────

function Shell({
  relay,
  soundEnabled,
  notificationsEnabled,
  onToggleSound,
  onToggleNotifications,
  onRefresh,
  wsConnected,
  children,
}: {
  relay: RelayStatus;
  soundEnabled: boolean;
  notificationsEnabled: boolean;
  onToggleSound: () => void;
  onToggleNotifications: () => void;
  onRefresh: () => void;
  wsConnected: boolean;
  children: React.ReactNode;
}) {
  const location = useLocation();
  const online = relay.status === 'ok';
  const isAgent = !!relay.this_agent_id;
  const navItems = [
    { to: '/', icon: <SlidersHorizontal size={16} />, label: isAgent ? 'Workspace' : 'Overview' },
    { to: '/inbox', icon: <InboxIcon size={16} />, label: isAgent ? 'Inbox' : 'Traffic', live: online },
    { to: '/agents', icon: <Server size={16} />, label: isAgent ? 'Connections' : 'Peers' },
    { to: '/setup', icon: <Settings2 size={16} />, label: isAgent ? 'Agent setup' : 'Connect' },
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
          <span className="brand-sub">{isAgent ? 'Agent Workspace' : 'Relay Console'}</span>
        </Link>
        <div className="topbar-right">
          {/* Connection status */}
          <div className={`connection-pill ${wsConnected ? 'connection-pill--live' : 'connection-pill--polling'}`}>
            {wsConnected ? <Wifi size={12} /> : <WifiOff size={12} />}
            <span>{wsConnected ? 'Live' : 'Polling'}</span>
          </div>
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
           <Button variant="icon" className="icon-btn" title={notificationsEnabled ? 'Disable desktop notifications' : 'Enable desktop notifications'} onClick={onToggleNotifications}>
            {notificationsEnabled ? <Bell size={15} /> : <BellOff size={15} />}
             <span>{notificationsEnabled ? 'Notify on' : 'Notify off'}</span>
           </Button>
           <Button variant="icon" className="icon-btn" title="Refresh data" aria-label="Refresh data" onClick={onRefresh}>
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
              aria-current={location.pathname === item.to ? 'page' : undefined}
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
          <p className="sidebar-note">
            {wsConnected ? 'Real-time updates active' : 'Polling every 10s'}
          </p>
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

function DashboardSkeleton() {
  return (
    <div className="dashboard-skeleton" aria-label="Loading dashboard" role="status">
      <div className="skeleton-block skeleton-block--banner" />
      <div className="skeleton-metrics">
        {[1, 2, 3, 4].map((item) => <div key={item} className="skeleton-block" />)}
      </div>
      <div className="skeleton-grid">
        <div className="skeleton-block skeleton-block--panel" />
        <div className="skeleton-block skeleton-block--panel" />
      </div>
      <span>Connecting to relay…</span>
    </div>
  );
}

function Dashboard({ data, onAgentClick }: { data: ReturnType<typeof useData>; onAgentClick?: (a: AgentHealth) => void }) {
  const { metrics, agents, dispatches, relay, loading, error, load, lastUpdated } = data;
  const recent = dispatches.slice(0, 8);
  const operational = relay.status === 'ok' && !error;
  const attentionItems = operational && metrics ? [
    { key: 'blocked', label: 'Blocked work', detail: 'Messages are blocked and need an owner.', count: metrics.blocked_work, tone: 'coral', to: '/inbox?filter=blocked' },
    { key: 'contract_drift', label: 'Contract drift', detail: 'Declared and actual behavior do not match.', count: metrics.contract_drift, tone: 'coral', to: '/inbox?filter=contract_drift' },
    { key: 'approval', label: 'Awaiting approval', detail: 'Work cannot proceed until approval is recorded.', count: metrics.awaiting_approvals, tone: 'amber', to: '/inbox?filter=approval' },
    { key: 'evidence', label: 'Evidence gaps', detail: 'Completed work is missing fresh evidence.', count: metrics.unverified_evidence, tone: 'amber', to: '/inbox?filter=evidence' },
    { key: 'offline', label: 'Offline agents', detail: 'Agents have not checked in recently.', count: metrics.offline_agents, tone: 'violet', to: '/agents?status=offline' },
  ].filter((item) => item.count > 0) : [];
  const alertMetrics = metrics
    ? [
        { label: 'Blocked work', value: metrics.blocked_work, accent: 'coral' },
        { label: 'Contract drift', value: metrics.contract_drift, accent: 'coral' },
        { label: 'Offline agents', value: metrics.offline_agents, accent: 'violet' },
        { label: 'Unverified evidence', value: metrics.unverified_evidence, accent: 'amber' },
        { label: 'Awaiting approvals', value: metrics.awaiting_approvals, accent: 'amber' },
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
        <div className="alert alert--error" role="alert">
          <AlertCircle size={16} />
          {error}
        </div>
      )}
      {loading && !metrics ? (
        <DashboardSkeleton />
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
            <p className="health-banner-freshness" aria-live="polite">
              {lastUpdated ? `Updated ${formatRelativeTime(lastUpdated)}` : 'Waiting for first update'}
            </p>
          </section>

          <section className={`attention-panel panel ${attentionItems.length === 0 ? 'attention-panel--clear' : ''}`}>
            <div className="panel-header">
              <div>
                <p className="eyebrow">Action required</p>
                <h2>Needs attention</h2>
              </div>
              <span className="attention-count">
                {attentionItems.length ? `${attentionItems.reduce((total, item) => total + item.count, 0)} items` : operational ? 'All clear' : 'Unavailable'}
              </span>
            </div>
            {attentionItems.length ? (
              <div className="attention-list">
                {attentionItems.map((item) => (
                  <Link key={item.key} className={`attention-item attention-item--${item.tone}`} to={item.to}>
                    <span className="attention-item-icon"><AlertCircle size={16} /></span>
                    <span className="attention-item-copy">
                      <strong>{item.label}</strong>
                      <small>{item.detail}</small>
                    </span>
                    <span className="attention-item-count">{item.count}</span>
                    <ArrowRight size={14} className="attention-item-arrow" />
                  </Link>
                ))}
              </div>
            ) : (
              <div className="attention-clear">
                {operational ? <Check size={18} /> : <AlertCircle size={18} />}
                <div>
                  <strong>{operational ? 'Nothing needs attention' : 'Waiting for relay data'}</strong>
                  <p>{operational ? 'The network is healthy and there are no outstanding blockers.' : 'Connect to the relay to load the operational queue.'}</p>
                </div>
              </div>
            )}
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
                <AgentFeed agents={agents} onAgentClick={onAgentClick} />
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

// Keep the original command-center implementation available for future
// per-agent views while the root route uses the relay-specific console below.
void Dashboard;

// ─── Relay Overview ──────────────────────────────────────────────────────────

function RelayDashboard({ data }: { data: ReturnType<typeof useData> }) {
  const { relay, dispatches, agents, loading, error, lastUpdated, load } = data;
  const operational = relay.status === 'ok' && !error;
  const onlineAgents = agents.filter((agent) => agent.status === 'online');
  const queued = dispatches.filter((dispatch) => ['dispatched', 'accepted'].includes(dispatch.status));
  const failed = dispatches.filter((dispatch) => ['blocked', 'delivery_failed'].includes(dispatch.status));
  const endpoint = import.meta.env.VITE_RELAY_URL || 'http://localhost:8443';

  const traffic = useMemo(() => {
    const buckets = Array.from({ length: 12 }, (_, index) => ({
      label: `${11 - index}h`,
      count: 0,
    }));
    const now = lastUpdated?.getTime() ?? 0;
    dispatches.forEach((dispatch) => {
      const age = Math.floor((now - new Date(dispatch.last_updated).getTime()) / 3_600_000);
      if (age >= 0 && age < 12) buckets[11 - age].count += 1;
    });
    return buckets;
  }, [dispatches, lastUpdated]);
  const maxTraffic = Math.max(1, ...traffic.map((bucket) => bucket.count));
  const recent = dispatches.slice(0, 6);

  const copyEndpoint = async () => {
    await navigator.clipboard.writeText(endpoint);
  };

  return (
    <div className="page relay-page">
      <PageHeader
        eyebrow="Relay / acp-relay"
        title="Message traffic, at a glance."
        description="A focused control room for the broker, connected peers, and every route moving through your ACP relay."
        actions={
          <>
            <Link className="btn btn--ghost" to="/inbox"><InboxIcon size={14} /> View traffic</Link>
            <Button onClick={() => void load()}><RefreshCw size={14} /> Refresh</Button>
          </>
        }
      />

      {error && (
        <div className="alert alert--error" role="alert">
          <AlertCircle size={16} /> {error}
        </div>
      )}

      {loading && !lastUpdated ? (
        <DashboardSkeleton />
      ) : (
        <>
          <section className={`relay-hero ${operational ? 'relay-hero--online' : 'relay-hero--offline'}`}>
            <div className="relay-hero-orbit" aria-hidden="true">
              <span className="orbit-ring orbit-ring--outer" />
              <span className="orbit-ring orbit-ring--inner" />
              <span className="orbit-core"><Radio size={25} /></span>
            </div>
            <div className="relay-hero-copy">
              <div className="relay-hero-kicker"><span className="status-dot status-dot--teal" /> Relay status</div>
              <h2>{operational ? 'Operational and accepting traffic' : 'Relay connection needs attention'}</h2>
              <p>{operational ? 'Messages can be forwarded to reachable peers or held safely in the broker.' : 'Check the container and shared secret, then refresh this view.'}</p>
              <div className="relay-hero-meta">
                <span><ShieldCheck size={14} /> Signed-token auth</span>
                <span><Activity size={14} /> ACP protocol v1.0</span>
                <span><Globe size={14} /> Port 8443</span>
              </div>
            </div>
            <div className="relay-hero-state">
              <StatusBadge value={operational ? 'online' : 'offline'} />
              <span>{lastUpdated ? `Updated ${formatRelativeTime(lastUpdated)}` : 'Waiting for data'}</span>
            </div>
          </section>

          <section className="relay-stat-grid" aria-label="Relay metrics">
            <div className="relay-stat-card relay-stat-card--accent">
              <div className="relay-stat-top"><span>Messages captured</span><MessageSquare size={16} /></div>
              <strong>{dispatches.length}</strong>
              <small>All relay traffic in the current store</small>
            </div>
            <div className="relay-stat-card">
              <div className="relay-stat-top"><span>Connected peers</span><Server size={16} /></div>
              <strong>{onlineAgents.length}<em> / {agents.length}</em></strong>
              <small>{agents.length ? 'Agents checked in recently' : 'Waiting for first agent'}</small>
            </div>
            <div className="relay-stat-card relay-stat-card--amber">
              <div className="relay-stat-top"><span>Broker queue</span><InboxIcon size={16} /></div>
              <strong>{queued.length}</strong>
              <small>Messages waiting for delivery or pickup</small>
            </div>
            <div className="relay-stat-card relay-stat-card--coral">
              <div className="relay-stat-top"><span>Delivery issues</span><AlertCircle size={16} /></div>
              <strong>{failed.length}</strong>
              <small>{failed.length ? 'Review traffic for failed routes' : 'No failed routes detected'}</small>
            </div>
          </section>

          <div className="relay-content-grid">
            <section className="panel traffic-chart-panel">
              <div className="panel-header">
                <div><p className="eyebrow">Throughput</p><h2>Traffic pulse</h2></div>
                <span className="panel-muted">Last 12 hours</span>
              </div>
              <div className="traffic-chart" aria-label="Messages captured over the last 12 hours">
                <div className="chart-y-axis"><span>{maxTraffic}</span><span>{Math.ceil(maxTraffic / 2)}</span><span>0</span></div>
                <div className="chart-bars">
                  <div className="chart-gridline chart-gridline--top" />
                  <div className="chart-gridline chart-gridline--middle" />
                  {traffic.map((bucket) => (
                    <div className="chart-bar-wrap" key={bucket.label}>
                      <div className={`chart-bar ${bucket.count ? 'chart-bar--active' : ''}`} style={{ height: `${Math.max(bucket.count ? 10 : 3, (bucket.count / maxTraffic) * 100)}%` }} title={`${bucket.count} message${bucket.count === 1 ? '' : 's'} ${bucket.label} ago`} />
                      <span>{bucket.label}</span>
                    </div>
                  ))}
                </div>
              </div>
              <div className="chart-footer"><span><span className="legend-dot legend-dot--teal" /> Captured messages</span><span>{dispatches.length} total</span></div>
            </section>

            <section className="panel endpoint-panel">
              <div className="panel-header">
                <div><p className="eyebrow">Connection</p><h2>Relay endpoint</h2></div>
                <Globe size={17} className="panel-icon" />
              </div>
              <div className="endpoint-visual"><span className="endpoint-line" /><span className="endpoint-node endpoint-node--left" /><span className="endpoint-node endpoint-node--right" /><span className="endpoint-pulse" /></div>
              <div className="endpoint-address"><code>{endpoint}</code><button className="copy-button" onClick={() => void copyEndpoint()} title="Copy relay endpoint" aria-label="Copy relay endpoint"><Copy size={14} /></button></div>
              <div className="endpoint-details">
                <div><span>Health</span><strong><span className={`status-dot status-dot--${operational ? 'teal' : 'coral'}`} /> {operational ? 'Reachable' : 'Unavailable'}</strong></div>
                <div><span>Identity</span><strong>{relay.agent || 'acp-relay'}</strong></div>
                <div><span>Transport</span><strong>HTTP + WebSocket</strong></div>
              </div>
              <Link className="endpoint-link" to="/setup">View connection setup <ArrowRight size={13} /></Link>
            </section>
          </div>

          <div className="relay-bottom-grid">
            <section className="panel recent-panel">
              <div className="panel-header"><div><p className="eyebrow">Live stream</p><h2>Recent routes</h2></div><Link className="btn btn--ghost btn--sm" to="/inbox">Open traffic <ArrowRight size={13} /></Link></div>
              {recent.length ? (
                <div className="relay-route-list">
                  {recent.map((dispatch) => (
                    <div className="relay-route-row" key={dispatch.dispatch_id}>
                      <span className={`route-intent route-intent--${dispatch.intent}`}><Send size={12} /></span>
                      <div className="relay-route-copy"><div><strong>{dispatch.from.agent_id}</strong><ArrowRight size={12} /><strong>{dispatch.to.agent_id}</strong></div><small>{dispatch.payload_preview || `${dispatch.intent} message`} · {formatRelativeTime(new Date(dispatch.last_updated))}</small></div>
                      <StatusBadge value={dispatch.status} />
                    </div>
                  ))}
                </div>
              ) : <div className="empty-state"><MessageSquare size={28} /><p>No routes yet</p><small>Messages will appear here as agents connect.</small></div>}
            </section>

            <section className="panel peers-panel">
              <div className="panel-header"><div><p className="eyebrow">Network</p><h2>Peer presence</h2></div><Link className="btn btn--ghost btn--sm" to="/agents">View peers <ArrowRight size={13} /></Link></div>
              {agents.length ? (
                <div className="relay-peer-list">
                  {agents.slice(0, 5).map((agent) => (
                    <div className="relay-peer-row" key={agent.peer.agent_id}><AgentAvatar agentId={agent.peer.agent_id} status={agent.status} /><div><strong>{agent.peer.agent_id}</strong><small>{agent.peer.machine_id}</small></div><span className="peer-last-seen"><StatusDot status={agent.status} /> {formatRelativeTime(agent.peer.last_seen_at)}</span></div>
                  ))}
                </div>
              ) : <div className="empty-state"><Server size={26} /><p>No peers registered</p></div>}
            </section>
          </div>
        </>
      )}
    </div>
  );
}

// ─── Agent Workspace ─────────────────────────────────────────────────────────

function AgentWorkspace({ data }: { data: ReturnType<typeof useData> }) {
  const { relay, dispatches, agents, loading, error, lastUpdated, load } = data;
  const identity = relay.this_agent_id || 'agent';
  const machine = relay.this_machine_id || 'local machine';
  const incoming = dispatches.filter((dispatch) => dispatch.to.agent_id === identity);
  const needsAction = incoming.filter((dispatch) => ['dispatched', 'accepted'].includes(dispatch.status));
  const active = dispatches.filter((dispatch) => ['in_progress', 'verification'].includes(dispatch.status));
  const completed = dispatches.filter((dispatch) => dispatch.status === 'complete');
  const failed = dispatches.filter((dispatch) => ['blocked', 'delivery_failed'].includes(dispatch.status));
  const relayPeer = agents.find((agent) => agent.is_relay);
  const otherPeers = agents.filter((agent) => !agent.is_relay);
  const recent = dispatches.slice(0, 7);

  return (
    <div className="page agent-page">
      <PageHeader
        eyebrow={`Agent / ${identity}`}
        title="Your work, in one place."
        description="See what arrived, what is moving, and where this agent is connected."
        actions={
          <>
            <Link className="btn btn--ghost" to="/setup"><Settings2 size={14} /> Configure</Link>
            <Link className="btn btn--primary" to="/inbox"><Send size={14} /> Send a message</Link>
          </>
        }
      />

      {error && <div className="alert alert--error" role="alert"><AlertCircle size={16} /> {error}</div>}

      {loading && !lastUpdated ? <DashboardSkeleton /> : (
        <>
          <section className={`agent-hero ${relay.status === 'ok' ? 'agent-hero--online' : 'agent-hero--offline'}`}>
            <div className="agent-identity-mark"><Cpu size={25} /><span /></div>
            <div className="agent-hero-copy">
              <div className="agent-hero-kicker"><span className={`status-dot status-dot--${relay.status === 'ok' ? 'teal' : 'coral'}`} /> Agent workspace</div>
              <h2>{identity}</h2>
              <p>{machine} · Ready to receive work from your network.</p>
              <div className="agent-hero-meta"><span><ShieldCheck size={14} /> ACP v1.0</span><span><Globe size={14} /> {relay.status === 'ok' ? 'Relay connected' : 'Relay unavailable'}</span><span><Clock3 size={14} /> {lastUpdated ? `Synced ${formatRelativeTime(lastUpdated)}` : 'Not synced'}</span></div>
            </div>
            <div className="agent-hero-actions"><StatusBadge value={relay.status === 'ok' ? 'online' : 'offline'} /><Link to="/setup" className="text-link">Connection details <ExternalLink size={12} /></Link></div>
          </section>

          <section className="agent-stat-grid" aria-label="Agent work metrics">
            <Link to="/inbox?filter=dispatched" className="agent-stat-card agent-stat-card--attention"><div><span>Needs attention</span><Target size={16} /></div><strong>{needsAction.length}</strong><small>{needsAction.length ? 'Incoming messages waiting for you' : 'Your inbox is clear'}</small></Link>
            <Link to="/inbox?filter=in_progress" className="agent-stat-card agent-stat-card--blue"><div><span>In progress</span><Activity size={16} /></div><strong>{active.length}</strong><small>{active.length ? 'Work currently moving through this agent' : 'No active work right now'}</small></Link>
            <Link to="/inbox?filter=complete" className="agent-stat-card agent-stat-card--teal"><div><span>Completed</span><CheckCircle2 size={16} /></div><strong>{completed.length}</strong><small>Completed messages in the current history</small></Link>
            <Link to="/inbox?filter=blocked" className="agent-stat-card agent-stat-card--coral"><div><span>Blocked</span><AlertCircle size={16} /></div><strong>{failed.length}</strong><small>{failed.length ? 'Routes need a retry or owner' : 'No blocked routes'}</small></Link>
          </section>

          <div className="agent-main-grid">
            <section className="panel agent-queue-panel">
              <div className="panel-header"><div><p className="eyebrow">Action queue</p><h2>Messages for you</h2></div><Link className="btn btn--ghost btn--sm" to="/inbox">Open inbox <ArrowRight size={13} /></Link></div>
              {needsAction.length ? <div className="agent-queue-list">{needsAction.slice(0, 5).map((dispatch) => <Link className="agent-queue-row" to="/inbox" key={dispatch.dispatch_id}><span className="queue-icon"><InboxIcon size={14} /></span><div><strong>{dispatch.from.agent_id}</strong><small>{dispatch.payload_preview || `${dispatch.intent} message`} · {formatRelativeTime(new Date(dispatch.last_updated))}</small></div><StatusBadge value={dispatch.status} /><ArrowRight size={14} /></Link>)}</div> : <div className="agent-empty"><CheckCircle2 size={27} /><div><strong>Nothing waiting for you</strong><p>New delegated work will appear here automatically.</p></div><Link className="btn btn--primary btn--sm" to="/inbox">View history</Link></div>}
            </section>

            <section className="panel agent-network-panel">
              <div className="panel-header"><div><p className="eyebrow">Connections</p><h2>Network</h2></div><Link className="btn btn--ghost btn--sm" to="/agents">Manage <ArrowRight size={13} /></Link></div>
              <div className="agent-connection-card"><div className="connection-orb"><Radio size={16} /></div><div><strong>Relay gateway</strong><small>{relayPeer?.peer.http_endpoint || 'Configured relay connection'}</small></div><StatusBadge value={relayPeer?.status === 'online' || relay.status === 'ok' ? 'online' : 'offline'} /></div>
              <div className="agent-network-summary"><span><strong>{otherPeers.length}</strong> peer{otherPeers.length === 1 ? '' : 's'}</span><span><strong>{otherPeers.filter((peer) => peer.status === 'online').length}</strong> online</span><span><strong>{dispatches.length}</strong> messages</span></div>
              <Link className="network-link" to="/setup"><ShieldCheck size={14} /> Review agent connection <ArrowRight size={13} /></Link>
            </section>
          </div>

          <section className="panel agent-activity-panel">
            <div className="panel-header"><div><p className="eyebrow">Activity</p><h2>Latest movement</h2></div><button className="btn btn--ghost btn--sm" onClick={() => void load()}><RefreshCw size={13} /> Refresh</button></div>
            {recent.length ? <div className="agent-activity-list">{recent.map((dispatch) => <div className="agent-activity-row" key={dispatch.dispatch_id}><span className={`activity-marker activity-marker--${dispatch.intent}`}><Send size={11} /></span><div className="agent-activity-route"><strong>{dispatch.from.agent_id}</strong><ArrowRight size={12} /><strong>{dispatch.to.agent_id}</strong><small>{dispatch.payload_preview || `${dispatch.intent} message`}</small></div><StatusBadge value={dispatch.status} /><span className="activity-time">{formatRelativeTime(new Date(dispatch.last_updated))}</span></div>)}</div> : <div className="empty-state"><MessageSquare size={28} /><p>No activity yet</p><small>Messages will appear as this agent connects.</small></div>}
          </section>
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

function AgentFeed({ agents, onAgentClick }: { agents: AgentHealth[]; onAgentClick?: (a: AgentHealth) => void }) {
  return (
    <div className="agent-feed">
      {agents.map((a) => (
        <button
          type="button"
          key={a.peer.agent_id}
          className="agent-feed-row agent-feed-row--clickable"
          onClick={() => onAgentClick?.(a)}
        >
          <AgentAvatar agentId={a.peer.agent_id} status={a.status} />
          <div className="agent-feed-info">
            <strong>{a.peer.agent_id}</strong>
            <small>{a.peer.machine_id} · {formatRelativeTime(a.peer.last_seen_at)}</small>
          </div>
          <div className="agent-feed-meta">
            <StatusBadge value={a.status} />
            {a.queue_depth > 0 && <span className="queue-badge">{a.queue_depth} queued</span>}
          </div>
        </button>
      ))}
    </div>
  );
}

// ─── Inbox ───────────────────────────────────────────────────────────────────

type FilterStatus = 'all' | Dispatch['status'];
type InboxFilter = FilterStatus | 'attention' | 'contract_drift' | 'approval' | 'evidence';
const PRIMARY_FILTERS: InboxFilter[] = ['all', 'attention', 'in_progress', 'complete', 'blocked', 'dispatched', 'verification'];
const SECONDARY_FILTERS: InboxFilter[] = ['contract_drift', 'approval', 'evidence'];

function Inbox({ data }: { data: ReturnType<typeof useData> }) {
  const [searchParams] = useSearchParams();
  const [query, setQuery] = useState('');
  const [inboxFilter, setInboxFilter] = useState<InboxFilter>(() => {
    const value = searchParams.get('filter') as InboxFilter | null;
    return value && [...PRIMARY_FILTERS, ...SECONDARY_FILTERS].includes(value) ? value : 'all';
  });
  const [selected, setSelected] = useState<Dispatch | null>(null);
  const [composeOpen, setComposeOpen] = useState(false);
  const [copiedId, setCopiedId] = useState(false);
  const [acknowledging, setAcknowledging] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const filtered = useMemo(() => {
    return data.dispatches.filter((item) => {
      const matchesQuery = query === '' ||
        `${item.from.agent_id} ${item.to.agent_id} ${item.intent} ${item.payload_preview ?? ''} ${item.status}`
          .toLowerCase()
          .includes(query.toLowerCase());
      const matchesFilter = inboxFilter === 'all' ||
        (inboxFilter === 'attention' && (
          ['blocked', 'delivery_failed'].includes(item.status) ||
          item.contract_status === 'drift' ||
          item.approval_state === 'required' ||
          ['missing', 'stale'].includes(item.evidence_status)
        )) ||
        (inboxFilter === 'contract_drift' && item.contract_status === 'drift') ||
        (inboxFilter === 'approval' && item.approval_state === 'required') ||
        (inboxFilter === 'evidence' && ['missing', 'stale'].includes(item.evidence_status)) ||
        (['in_progress', 'complete', 'blocked', 'dispatched', 'verification'].includes(inboxFilter) && item.status === inboxFilter);
      return matchesQuery && matchesFilter;
    });
  }, [data.dispatches, inboxFilter, query]);

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

  const acknowledge = async () => {
    if (!selected) return;
    setAcknowledging(true);
    setActionError(null);
    try {
      await acknowledgeMessage(selected.dispatch_id);
      await data.load();
      setSelected(null);
    } catch (value) {
      setActionError(value instanceof Error ? value.message : 'Unable to acknowledge message');
    } finally {
      setAcknowledging(false);
    }
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
                aria-label="Search messages"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search agents, intents, content…"
                className="search-input"
              />
            </div>
          </div>
          <div className="filter-toolbar">
            <div className="filter-chips" aria-label="Message filters">
            {PRIMARY_FILTERS.map((filter) => (
              <button
                key={filter}
                className={`filter-chip ${inboxFilter === filter ? 'filter-chip--active' : ''}`}
                aria-pressed={inboxFilter === filter}
                onClick={() => setInboxFilter(filter)}
              >
                {filter === 'all' ? 'All' : filter === 'attention' ? 'Needs attention' : STATUS_CONFIG[filter]?.label ?? filter}
              </button>
            ))}
            </div>
            <select
              className="filter-select filter-select--secondary"
              aria-label="More message filters"
              value={SECONDARY_FILTERS.includes(inboxFilter) ? inboxFilter : ''}
              onChange={(event) => setInboxFilter((event.target.value || 'all') as InboxFilter)}
            >
              <option value="">More filters</option>
              <option value="contract_drift">Contract drift</option>
              <option value="approval">Awaiting approval</option>
              <option value="evidence">Evidence gaps</option>
            </select>
            <select
              className="filter-select filter-select--mobile"
              aria-label="Filter messages"
              value={inboxFilter}
              onChange={(event) => setInboxFilter(event.target.value as InboxFilter)}
            >
              <option value="all">All</option>
              <option value="attention">Needs attention</option>
              <option value="in_progress">Running</option>
              <option value="complete">Complete</option>
              <option value="blocked">Blocked</option>
              <option value="dispatched">Sent</option>
              <option value="verification">Verify</option>
              <option value="contract_drift">Contract drift</option>
              <option value="approval">Awaiting approval</option>
              <option value="evidence">Evidence gaps</option>
            </select>
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
            <div className="panel detail-panel" role="dialog" aria-modal="true" aria-labelledby="message-detail-title">
              <div className="detail-header">
                <div>
                  <p className="eyebrow">Message detail</p>
                  <h2 id="message-detail-title">{selected.intent} message</h2>
                </div>
                <button className="icon-btn icon-btn--ghost" aria-label="Close message details" onClick={() => setSelected(null)}>
                  <X size={16} />
                </button>
              </div>
              <div className="detail-meta">
                <StatusBadge value={selected.status} />
                <code className="detail-id">{selected.dispatch_id}</code>
                {['dispatched', 'accepted'].includes(selected.status) && (
                  <button className="btn btn--primary btn--sm" onClick={() => void acknowledge()} disabled={acknowledging}>
                    <Check size={12} />
                    {acknowledging ? 'Acknowledging…' : 'Acknowledge'}
                  </button>
                )}
                <button className="btn btn--ghost btn--sm" onClick={copyId}>
                  {copiedId ? <Check size={12} /> : <Clipboard size={12} />}
                  {copiedId ? 'Copied' : 'Copy ID'}
                </button>
              </div>
              {actionError && <div className="detail-action-error" role="alert">{actionError}</div>}
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
                  <div className="payload-body">
                    {selected.payload_content.startsWith('{') || selected.payload_content.startsWith('[')
                      ? <pre className="json-viewer"><code>{selected.payload_content}</code></pre>
                      : <Markdown remarkPlugins={[remarkGfm]}>{selected.payload_content}</Markdown>
                    }
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

function Agents({ data, onAgentClick }: { data: ReturnType<typeof useData>; onAgentClick?: (a: AgentHealth) => void }) {
  const [searchParams] = useSearchParams();
  const { agents, loading } = data;
  const online = agents.filter((a) => a.status === 'online');
  const offline = agents.filter((a) => a.status === 'offline');
  const statusFilter = searchParams.get('status');
  const visibleAgents = statusFilter === 'offline' ? offline : statusFilter === 'online' ? online : agents;

  return (
    <div className="page">
      <PageHeader
        eyebrow="Network"
        title="Agents"
        description={`${visibleAgents.length} shown of ${agents.length} registered — ${online.length} online, ${offline.length} offline${statusFilter ? ` · filtered to ${statusFilter}` : ''}.`}
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
      ) : visibleAgents.length === 0 ? (
        <div className="panel">
          <div className="empty-state">
            <Server size={36} />
            <h3>{statusFilter ? `No ${statusFilter} agents` : 'No agents found'}</h3>
            <p>{statusFilter ? 'Try clearing the current agent filter.' : 'Agents will appear here once they connect to the relay.'}</p>
          </div>
        </div>
      ) : (
        <div className="agents-grid">
          {visibleAgents.map((a) => (
            <button
              type="button"
              key={a.peer.agent_id}
              className={`agent-card ${a.status === 'offline' ? 'agent-card--offline' : ''}`}
              onClick={() => onAgentClick?.(a)}
            >
              <div className="agent-card-header">
                <AgentAvatar agentId={a.peer.agent_id} status={a.status} />
                <div className="agent-card-info">
                  <strong>{a.peer.agent_id}</strong>
                  <small>{a.peer.machine_id} · last seen {formatRelativeTime(a.peer.last_seen_at)}</small>
                </div>
                <StatusBadge value={a.status} />
              </div>
              <div className="agent-card-details">
                <div className="agent-card-detail">
                  <Globe size={13} />
                  <code>{a.peer.http_endpoint || 'No endpoint'}</code>
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
            </button>
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
  relayUrl: 'http://localhost:8443',
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

// ─── Agent Detail Modal ──────────────────────────────────────────────────────

function AgentDetailModal({
  agent,
  onClose,
  onMessage,
}: {
  agent: AgentHealth;
  onClose: () => void;
  onMessage: (agentId: string) => void;
}) {
  const isOnline = agent.status === 'online';
  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}
    >
      <section className="agent-detail-modal" role="dialog" aria-modal="true" aria-labelledby="agent-title">
        <div className="modal-header">
          <div className="agent-detail-header">
            <AgentAvatar agentId={agent.peer.agent_id} status={agent.status} />
            <div>
              <p className="eyebrow">Agent</p>
              <h2 id="agent-title">{agent.peer.agent_id}</h2>
            </div>
            <StatusBadge value={agent.status} />
          </div>
          <button className="icon-btn icon-btn--ghost" onClick={onClose} aria-label="Close">
            <X size={18} />
          </button>
        </div>

        <div className="agent-detail-body">
          {/* Identity */}
          <div className="detail-section">
            <h3><Globe size={14} /> Identity</h3>
            <div className="detail-grid">
              <div className="detail-item">
                <span className="detail-item-label">Agent ID</span>
                <code>{agent.peer.agent_id}</code>
              </div>
              <div className="detail-item">
                <span className="detail-item-label">Machine ID</span>
                <code>{agent.peer.machine_id}</code>
              </div>
              {agent.peer.http_endpoint && (
                <div className="detail-item detail-item--full">
                  <span className="detail-item-label">HTTP Endpoint</span>
                  <code>{agent.peer.http_endpoint}</code>
                </div>
              )}
              {agent.peer.ws_endpoint && (
                <div className="detail-item detail-item--full">
                  <span className="detail-item-label">WebSocket Endpoint</span>
                  <code>{agent.peer.ws_endpoint}</code>
                </div>
              )}
            </div>
          </div>

          {/* Capabilities */}
          {agent.peer.capabilities && agent.peer.capabilities.length > 0 && (
            <div className="detail-section">
              <h3><Zap size={14} /> Capabilities</h3>
              <div className="capability-list">
                {agent.peer.capabilities.map((cap) => (
                  <span key={cap} className="capability-chip capability-chip--large">{cap}</span>
                ))}
              </div>
            </div>
          )}

          {/* Metrics */}
          <div className="detail-section">
            <h3><Activity size={14} /> Metrics</h3>
            <div className="detail-grid">
              <div className="detail-item">
                <span className="detail-item-label">Queue Depth</span>
                <span className="detail-item-value">{agent.queue_depth}</span>
              </div>
              <div className="detail-item">
                <span className="detail-item-label">Retry Count</span>
                <span className="detail-item-value">{agent.retry_count}</span>
              </div>
              <div className="detail-item">
                <span className="detail-item-label">CDF Context</span>
                <StatusBadge value={agent.cdf_context_freshness ?? 'unknown'} />
              </div>
              <div className="detail-item">
                <span className="detail-item-label">Version Compatible</span>
                <StatusBadge value={agent.version_compatible ? 'online' : 'offline'} />
              </div>
            </div>
          </div>

          {/* Actions */}
          <div className="detail-section detail-section--actions">
            <Button
              variant="primary"
              onClick={() => { onClose(); onMessage(agent.peer.agent_id); }}
              disabled={!isOnline}
            >
              <Send size={14} />
              {isOnline ? `Send message to ${agent.peer.agent_id}` : 'Agent is offline'}
            </Button>
          </div>
        </div>
      </section>
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
  const [notificationsEnabled, setNotificationsEnabled] = useState(
    () => localStorage.getItem('acp-notifications-enabled') === 'true'
  );
  const [soundEnabled, setSoundEnabled] = useState(
    () => localStorage.getItem('acp-sound-enabled') === 'true'
  );
  const [composeRecipient, setComposeRecipient] = useState<string | null>(null);
  const [detailAgent, setDetailAgent] = useState<AgentHealth | null>(null);
  const [announcement, setAnnouncement] = useState('');
  const audioContext = useRef<AudioContext | null>(null);
  const seenMessages = useRef<Set<string> | null>(null);

  const { notify, requestPermission } = useNotifications(notificationsEnabled);

  const playSound = () => {
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

  const handleNewMessage = (dispatch: Dispatch) => {
    setAnnouncement(`New ${dispatch.intent} message from ${dispatch.from.agent_id}`);
    if (soundEnabled) playSound();
    if (notificationsEnabled) {
      notify(
        `New ${dispatch.intent} from ${dispatch.from.agent_id}`,
        dispatch.payload_preview ?? `Message to ${dispatch.to.agent_id}`,
      );
    }
  };

  const data = useData({ onNewMessage: handleNewMessage });

  useEffect(() => {
    if (data.loading) return;
    const currentIds = new Set(data.dispatches.map((message) => message.dispatch_id));
    if (seenMessages.current && soundEnabled && [...currentIds].some((id) => !seenMessages.current?.has(id))) {
      playSound();
    }
    seenMessages.current = currentIds;
  }, [data.dispatches, data.loading, soundEnabled]);

  const toggleSound = () => {
    const next = !soundEnabled;
    setSoundEnabled(next);
    localStorage.setItem('acp-sound-enabled', String(next));
    if (next) playSound();
  };

  const toggleNotifications = () => {
    if (!notificationsEnabled) {
      requestPermission();
    }
    const next = !notificationsEnabled;
    setNotificationsEnabled(next);
    localStorage.setItem('acp-notifications-enabled', String(next));
  };

  const openDetail = (agent: AgentHealth) => setDetailAgent(agent);
  const closeDetail = () => setDetailAgent(null);
  const openCompose = (agentId?: string) => {
    setComposeRecipient(agentId ?? null);
  };
  const closeCompose = () => setComposeRecipient(null);

  return (
    <BrowserRouter>
      <div className="sr-only" aria-live="polite" aria-atomic="true">{announcement}</div>
      <Shell
        relay={data.relay}
        soundEnabled={soundEnabled}
        notificationsEnabled={notificationsEnabled}
        onToggleSound={toggleSound}
        onToggleNotifications={toggleNotifications}
        onRefresh={() => void data.load()}
        wsConnected={data.wsConnected}
      >
        <Routes>
          <Route path="/" element={data.relay.this_agent_id ? <AgentWorkspace data={data} /> : <RelayDashboard data={data} />} />
          <Route path="/inbox" element={<Inbox data={data} />} />
          <Route path="/agents" element={<Agents data={data} onAgentClick={openDetail} />} />
          <Route path="/setup" element={<Setup />} />
        </Routes>
      </Shell>

      {/* Agent detail modal */}
      {detailAgent && (
        <AgentDetailModal
          agent={detailAgent}
          onClose={closeDetail}
          onMessage={(agentId) => { closeDetail(); openCompose(agentId); }}
        />
      )}

      {/* Compose modal (triggered from agent detail or inbox) */}
      {composeRecipient !== null && (
        <ComposeModalWrapper
          initialRecipient={composeRecipient}
          onClose={closeCompose}
          onSent={() => { closeCompose(); void data.load(); }}
          agents={data.agents}
          thisAgentId={data.relay.this_agent_id || 'naiplawan-agent'}
          thisMachineId={data.relay.this_machine_id || 'naiplawan-machine'}
        />
      )}
    </BrowserRouter>
  );
}

// Wrapper to open compose with pre-filled recipient
function ComposeModalWrapper({
  initialRecipient,
  onClose,
  onSent,
  agents,
  thisAgentId,
  thisMachineId,
}: {
  initialRecipient: string;
  onClose: () => void;
  onSent: () => void;
  agents: AgentHealth[];
  thisAgentId: string;
  thisMachineId: string;
}) {
  const [recipient, setRecipient] = useState(initialRecipient);
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
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}
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

export default App;
