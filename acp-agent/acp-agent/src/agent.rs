//! ACP Agent — send/receive/delegate, reply routing, and streaming
//!
//! Includes ACP-CHP: Context Handoff Protocol support.

use std::collections::HashMap;
use std::sync::Arc;

use acp_core::chp::{ContextBundle, HandoffIntent, HandoffMessage};
use acp_core::config::{load_config, resolve_this_agent, ACPConfig, Peer, ThisAgent};
use acp_core::protocol::{
    build_envelope, forward_envelope, new_corr_id, new_msg_id, new_stream_id, reply_envelope,
    Intent, Message, NewEnvelope, Origin, Priority, DEFAULT_MAX_HOPS,
};
use acp_core::transport::ACPHttpClient;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::error::AgentError;

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// Where an agent is in handling the message it is currently working on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentState {
    /// Nothing in flight.
    #[default]
    Idle,
    /// A message has arrived but has not been dispatched.
    Received,
    /// A message is being passed to another agent.
    Forwarding,
    /// A handler is running.
    Processing,
    /// An answer is being sent back along the reply path.
    Replying,
    /// The message was handled through to its reply.
    Complete,
    /// The message could not be handled or its reply could not be delivered.
    Failed,
}

impl AgentState {
    /// Wire representation of this state.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentState::Idle => "idle",
            AgentState::Received => "received",
            AgentState::Forwarding => "forwarding",
            AgentState::Processing => "processing",
            AgentState::Replying => "replying",
            AgentState::Complete => "complete",
            AgentState::Failed => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Pending message tracking
// ---------------------------------------------------------------------------

/// One received message, retained for the dashboard.
#[derive(Debug, Clone)]
pub struct InboxEntry {
    /// Message ID.
    pub msg_id: String,
    /// Correlation ID of the exchange.
    pub corr_id: String,
    /// User or automation conversation this message belongs to.
    pub session_id: Option<String>,
    /// One execution within the session.
    pub run_id: Option<String>,
    /// Sending agent.
    pub sender_agent: String,
    /// Machine the sender ran on.
    pub sender_machine: String,
    /// Receiving agent.
    pub recipient_agent: String,
    /// Machine the recipient ran on.
    pub recipient_machine: String,
    /// Wire form of the message intent.
    pub intent: String,
    /// Application data, when the message carried any.
    pub payload: Option<Value>,
    /// Where the message stands.
    pub status: String,
    /// Failure description, empty when there was none.
    pub error: String,
    /// Unix seconds when the message arrived.
    pub received_at: f64,
}

/// One message this agent is still working on.
#[derive(Debug, Clone)]
pub struct PendingMessage {
    /// The message itself.
    pub message: Message,
    /// Where handling stands.
    pub state: AgentState,
    /// Unix seconds when tracking began.
    pub created_at: f64,
    /// Stream opened to answer it, when one was.
    pub stream_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Message handler type
// ---------------------------------------------------------------------------

/// A registered handler. Returning `Some` triggers an auto-reply.
pub type MessageHandler = Arc<dyn Fn(Message) -> HandlerResult + Send + Sync>;

/// What a handler produces: an optional reply payload.
pub type HandlerResult = Option<Value>;

// ---------------------------------------------------------------------------
// ACPAgent
// ---------------------------------------------------------------------------

/// An ACP participant: routing config, an HTTP client, and registered handlers.
///
/// Cloning is not supported; share one behind an [`Arc`] instead. Every field is
/// individually locked, so handlers may run concurrently.
pub struct ACPAgent {
    /// Peer config this agent routes with.
    pub config: Arc<ACPConfig>,
    /// This agent's own identity and endpoints.
    pub this: ThisAgent,
    /// Signed HTTP client used for every outbound message.
    pub client: Arc<ACPHttpClient>,
    handlers: Arc<RwLock<HashMap<String, MessageHandler>>>,
    handoff_handlers: Arc<RwLock<HashMap<String, MessageHandler>>>,
    pending: Arc<RwLock<HashMap<String, PendingMessage>>>,
    inbox: Arc<RwLock<Vec<InboxEntry>>>,
    state: Arc<RwLock<AgentState>>,
}

impl ACPAgent {
    // ---- Construction ----

    /// Load config from `config_path`, the environment, or the default locations.
    ///
    /// # Errors
    /// Returns [`AgentError::Config`] when no config resolves or `this_agent` is
    /// incomplete.
    pub fn from_config_file(config_path: Option<&str>) -> Result<Self, AgentError> {
        Self::from_config(load_config(config_path)?)
    }

    /// Construct from a pre-loaded config.
    ///
    /// # Errors
    /// Returns [`AgentError::Config`] when `this_agent` is missing or has no
    /// `http_endpoint`.
    pub fn from_config(config: ACPConfig) -> Result<Self, AgentError> {
        let this = resolve_this_agent(&config)?;

        let client = ACPHttpClient::new(
            config.clone(),
            this.agent_id.clone(),
            this.machine_id.clone().unwrap_or_default(),
        );

        Ok(Self {
            config: Arc::new(config),
            this,
            client: Arc::new(client),
            handlers: Arc::new(RwLock::new(HashMap::new())),
            handoff_handlers: Arc::new(RwLock::new(HashMap::new())),
            pending: Arc::new(RwLock::new(HashMap::new())),
            inbox: Arc::new(RwLock::new(Vec::new())),
            state: Arc::new(RwLock::new(AgentState::Idle)),
        })
    }

    /// This agent's address as `agent_id@machine_id`.
    fn addr(&self) -> String {
        format!("{}@{}", self.this.agent_id, self.machine_id())
    }

    fn machine_id(&self) -> &str {
        self.this.machine_id.as_deref().unwrap_or_default()
    }

    // ---- Handler registration ----

    /// Register a handler for [`Intent::Delegate`] messages.
    pub async fn on_delegate<F>(&self, f: F)
    where
        F: Fn(Message) -> HandlerResult + Send + Sync + 'static,
    {
        self.register(Intent::Delegate.as_str(), Arc::new(f)).await;
    }

    /// Register a handler for [`Intent::Reply`] messages.
    pub async fn on_reply<F>(&self, f: F)
    where
        F: Fn(Message) -> HandlerResult + Send + Sync + 'static,
    {
        self.register(Intent::Reply.as_str(), Arc::new(f)).await;
    }

    /// Register a handler for [`Intent::Error`] messages.
    pub async fn on_error<F>(&self, f: F)
    where
        F: Fn(Message) -> HandlerResult + Send + Sync + 'static,
    {
        self.register(Intent::Error.as_str(), Arc::new(f)).await;
    }

    async fn register(&self, intent: &str, handler: MessageHandler) {
        self.handlers
            .write()
            .await
            .insert(intent.to_string(), handler);
    }

    /// Register a handler for CHP handoff messages.
    ///
    /// The handler is skipped for a message whose payload carries no readable
    /// [`ContextBundle`].
    pub async fn on_handoff<F>(&self, f: F)
    where
        F: Fn(ContextBundle, Message) -> HandlerResult + Send + Sync + 'static,
    {
        self.register_handoff(HandoffIntent::Handoff, f).await;
    }

    /// Register a handler for CHP handover-request messages.
    ///
    /// The handler is skipped for a message whose payload carries no readable
    /// [`ContextBundle`].
    pub async fn on_handover_request<F>(&self, f: F)
    where
        F: Fn(ContextBundle, Message) -> HandlerResult + Send + Sync + 'static,
    {
        self.register_handoff(HandoffIntent::HandoverRequest, f)
            .await;
    }

    async fn register_handoff<F>(&self, intent: HandoffIntent, f: F)
    where
        F: Fn(ContextBundle, Message) -> HandlerResult + Send + Sync + 'static,
    {
        let handler: MessageHandler = Arc::new(move |msg| {
            let bundle = extract_bundle_from_payload(msg.payload.as_ref())?;
            f(bundle, msg)
        });
        self.handoff_handlers
            .write()
            .await
            .insert(intent.as_str().to_string(), handler);
    }

    // ---- Send operations ----

    /// Send a new message to a peer, starting a fresh chain.
    ///
    /// # Errors
    /// Returns [`AgentError::UnknownPeer`] when `target_agent_id` is not
    /// configured, or [`AgentError::Transport`] when delivery fails.
    pub async fn send_message(
        &self,
        target_agent_id: &str,
        payload: Value,
        origin: Option<Origin>,
    ) -> Result<Message, AgentError> {
        let peer = self
            .config
            .get_peer(target_agent_id)
            .ok_or_else(|| AgentError::UnknownPeer(target_agent_id.to_string()))?;

        let envelope = build_envelope(NewEnvelope {
            msg_id: new_msg_id(),
            corr_id: new_corr_id(),
            session_id: Some(acp_core::protocol::new_session_id()),
            run_id: Some(acp_core::protocol::new_run_id()),
            origin: origin.unwrap_or_else(|| Origin {
                agent_id: Some(self.this.agent_id.clone()),
                machine_id: self.this.machine_id.clone(),
                human_id: None,
            }),
            sender: (&self.this.agent_id, self.machine_id()),
            recipient: (&peer.agent_id, &peer.machine_id),
            intent: Intent::Delegate,
            reply_to_path: Some(vec![self.addr()]),
            reply_to_ws_endpoint: self.this.ws_endpoint.clone(),
            hops_max: DEFAULT_MAX_HOPS,
            content_type: "application/json",
            priority: Priority::Normal,
            deadline: None,
        });

        let message = Message::with_payload(envelope, payload);
        self.client.send_with_retry(peer, &message).await?;
        Ok(message)
    }

    /// Send a CHP handoff bundle to another agent.
    ///
    /// # Errors
    /// Returns [`AgentError::Serialization`] when the bundle cannot be encoded,
    /// or whatever [`ACPAgent::send_message`] returns.
    pub async fn send_handoff(
        &self,
        target_agent_id: &str,
        bundle: ContextBundle,
        origin: Option<Origin>,
    ) -> Result<Message, AgentError> {
        let handoff = HandoffMessage::new(bundle, HandoffIntent::Handoff);
        let payload = serde_json::to_value(&handoff)?;
        self.send_message(target_agent_id, payload, origin).await
    }

    /// Forward an incoming message to another agent, extending the reply path.
    ///
    /// # Errors
    /// Returns [`AgentError::UnknownPeer`] when the target is not configured,
    /// [`AgentError::HopsExceeded`] when the chain is out of hops, or
    /// [`AgentError::Transport`] when delivery fails.
    pub async fn delegate_to(
        &self,
        target_agent_id: &str,
        original_message: &Message,
        additional_payload: Option<Value>,
    ) -> Result<Message, AgentError> {
        let peer = self
            .config
            .get_peer(target_agent_id)
            .ok_or_else(|| AgentError::UnknownPeer(target_agent_id.to_string()))?;

        let new_envelope = forward_envelope(
            &original_message.envelope,
            &self.this.agent_id,
            self.machine_id(),
            &peer.agent_id,
            &peer.machine_id,
        )?;

        let merged_payload = match additional_payload {
            Some(add) => merge_payload(original_message.payload.as_ref(), add),
            None => original_message.payload.clone().unwrap_or(Value::Null),
        };

        let new_message = Message::with_payload(new_envelope, merged_payload);
        self.track_pending(
            &original_message.envelope.msg_id,
            &new_message,
            AgentState::Forwarding,
        )
        .await;

        self.client.send_with_retry(peer, &new_message).await?;
        Ok(new_message)
    }

    /// Send a reply back along `reply_to.path`.
    ///
    /// # Errors
    /// Returns [`AgentError::ReplyPathEmpty`] when there is nowhere to reply to,
    /// [`AgentError::UnroutableReply`] when the next hop is not a configured
    /// peer, or [`AgentError::Transport`] when delivery fails.
    pub async fn reply_to(
        &self,
        original_message: &Message,
        payload: Value,
    ) -> Result<Message, AgentError> {
        let new_envelope = reply_envelope(
            &original_message.envelope,
            &self.this.agent_id,
            self.machine_id(),
        )?;

        let peer = self.resolve_reply_peer(&new_envelope.recipient.to_str(), "reply")?;

        let new_message = Message::with_payload(new_envelope, payload);
        self.track_pending(
            &original_message.envelope.msg_id,
            &new_message,
            AgentState::Replying,
        )
        .await;

        self.client.send_with_retry(peer, &new_message).await?;
        Ok(new_message)
    }

    /// Propagate a failure back along the reply path.
    ///
    /// # Errors
    /// The same failures as [`ACPAgent::reply_to`].
    pub async fn send_error(
        &self,
        original_message: &Message,
        error: &str,
        retryable: bool,
    ) -> Result<Message, AgentError> {
        let mut new_envelope = reply_envelope(
            &original_message.envelope,
            &self.this.agent_id,
            self.machine_id(),
        )?;
        new_envelope.error = Some(error.to_string());
        new_envelope.intent = Intent::Error;

        let peer = self.resolve_reply_peer(&new_envelope.recipient.to_str(), "error")?;

        let error_payload = serde_json::json!({
            "error": error,
            "retryable": retryable,
            "failed_at": self.addr(),
        });

        let new_message = Message::with_payload(new_envelope, error_payload);
        self.client.send_with_retry(peer, &new_message).await?;
        Ok(new_message)
    }

    fn resolve_reply_peer(&self, addr: &str, context: &'static str) -> Result<&Peer, AgentError> {
        self.config
            .get_peer_by_addr(addr)
            .ok_or_else(|| AgentError::UnroutableReply {
                context,
                addr: addr.to_string(),
            })
    }

    // ---- Streaming ----

    /// Open a stream for answering `original_message`, and return its ID.
    pub async fn initiate_stream(&self, original_message: &Message) -> String {
        let stream_id = new_stream_id();
        let mut pending = self.pending.write().await;
        let entry = pending
            .entry(original_message.envelope.msg_id.clone())
            .or_insert_with(|| PendingMessage {
                message: original_message.clone(),
                state: AgentState::Processing,
                created_at: now_f64(),
                stream_id: None,
            });
        entry.stream_id = Some(stream_id.clone());
        stream_id
    }

    /// Send one chunk of a streamed reply toward the origin.
    ///
    /// A message with an empty reply path has nowhere to stream to; that is
    /// logged and treated as a no-op rather than an error.
    ///
    /// # Errors
    /// Returns [`AgentError::UnroutableReply`] when the next hop is not a
    /// configured peer, or [`AgentError::Transport`] when delivery fails.
    pub async fn send_stream_chunk(
        &self,
        original_message: &Message,
        stream_id: &str,
        seq: u32,
        total: Option<u32>,
        data: Value,
        final_: bool,
    ) -> Result<(), AgentError> {
        let first_hop = original_message
            .envelope
            .reply_to
            .as_ref()
            .and_then(|rt| rt.path.first().cloned());

        let Some(next_hop_addr) = first_hop else {
            tracing::warn!("No reply_to.path for stream {stream_id}");
            return Ok(());
        };

        let peer = self.resolve_reply_peer(&next_hop_addr, "stream")?;

        let chunk_envelope = reply_envelope(
            &original_message.envelope,
            &self.this.agent_id,
            self.machine_id(),
        )?;

        let chunk_message = Message::with_payload(
            chunk_envelope,
            serde_json::json!({
                "stream_id": stream_id,
                "seq": seq,
                "total": total,
                "data": data,
                "final": final_,
            }),
        );

        self.client.send_with_retry(peer, &chunk_message).await?;
        Ok(())
    }

    /// Send a reply as a stream of chunks, then a summary reply.
    ///
    /// # Errors
    /// The same failures as [`ACPAgent::send_stream_chunk`] and
    /// [`ACPAgent::reply_to`].
    pub async fn stream_reply(
        &self,
        original_message: &Message,
        chunks: Vec<(Value, bool)>,
    ) -> Result<Message, AgentError> {
        let stream_id = self.initiate_stream(original_message).await;
        let total = u32::try_from(chunks.len()).unwrap_or(u32::MAX);

        for (seq, (data, final_)) in chunks.into_iter().enumerate() {
            self.send_stream_chunk(
                original_message,
                &stream_id,
                u32::try_from(seq).unwrap_or(u32::MAX),
                Some(total),
                data,
                final_,
            )
            .await?;
        }

        let summary_payload = serde_json::json!({
            "stream_id": stream_id,
            "total_frames": total,
            "origin": self.addr(),
        });

        self.reply_to(original_message, summary_payload).await
    }

    // ---- Message processing ----

    /// Dispatch a message to its handler.
    ///
    /// A payload carrying a CHP `intent` is offered to the handoff handlers
    /// first; otherwise the envelope's intent selects the handler. Returns the
    /// handler's reply payload, or `None` when nothing is registered.
    pub async fn process_message(&self, message: Message) -> HandlerResult {
        *self.state.write().await = AgentState::Processing;

        if let Some(intent) = message
            .payload
            .as_ref()
            .and_then(|p| p.get("intent"))
            .and_then(Value::as_str)
        {
            let handoff_handlers = self.handoff_handlers.read().await;
            if let Some(handler) = handoff_handlers.get(intent) {
                tracing::debug!("Dispatching CHP intent {intent}");
                return handler(message);
            }
        }

        let intent = message.envelope.intent.as_str();
        let handlers = self.handlers.read().await;
        let Some(handler) = handlers.get(intent) else {
            tracing::warn!("No handler for intent: {intent}");
            return None;
        };
        handler(message)
    }

    /// Record an incoming message, dispatch it, and auto-reply when the handler
    /// returns a payload and the message carries a reply path.
    pub async fn handle_incoming(&self, message: Message) {
        {
            let mut pending = self.pending.write().await;
            if pending.contains_key(&message.envelope.msg_id) {
                tracing::debug!(
                    msg_id = %message.envelope.msg_id,
                    "ignoring duplicate incoming message"
                );
                return;
            }
            pending.insert(
                message.envelope.msg_id.clone(),
                PendingMessage {
                    message: message.clone(),
                    state: AgentState::Received,
                    created_at: now_f64(),
                    stream_id: None,
                },
            );
        }
        self.record_inbox(&message).await;
        *self.state.write().await = AgentState::Received;

        let result = self.process_message(message.clone()).await;

        let mut final_state = AgentState::Complete;
        if let Some(result_payload) = result {
            if message.envelope.reply_to.is_some() {
                if let Err(e) = self.reply_to(&message, result_payload).await {
                    tracing::warn!("Auto-reply failed: {e}");
                    final_state = AgentState::Failed;
                }
            }
        }

        *self.state.write().await = final_state;
    }

    async fn record_inbox(&self, message: &Message) {
        let envelope = &message.envelope;
        self.inbox.write().await.push(InboxEntry {
            msg_id: envelope.msg_id.clone(),
            corr_id: envelope.corr_id.clone().unwrap_or_default(),
            session_id: envelope.session_id.clone(),
            run_id: envelope.run_id.clone(),
            sender_agent: envelope.sender.agent_id.clone(),
            sender_machine: envelope.sender.machine_id.clone().unwrap_or_default(),
            recipient_agent: envelope.recipient.agent_id.clone(),
            recipient_machine: envelope.recipient.machine_id.clone().unwrap_or_default(),
            intent: envelope.intent.as_str().to_string(),
            payload: message.payload.clone(),
            status: "received".to_string(),
            error: envelope.error.clone().unwrap_or_default(),
            received_at: now_f64(),
        });
    }

    async fn track_pending(&self, key: &str, message: &Message, state: AgentState) {
        self.pending.write().await.insert(
            key.to_string(),
            PendingMessage {
                message: message.clone(),
                state,
                created_at: now_f64(),
                stream_id: None,
            },
        );
    }

    // ---- Server lifecycle ----

    /// Run until interrupted.
    ///
    /// HTTP serving lives in [`crate::server::start_server`]; this only holds the
    /// process open and tracks agent state.
    ///
    /// # Errors
    /// Returns [`AgentError::Io`] when the interrupt handler cannot be installed.
    pub async fn run(&self) -> Result<(), AgentError> {
        *self.state.write().await = AgentState::Idle;
        tracing::info!("[{}] ACP Agent running", self.this.agent_id);

        tokio::signal::ctrl_c().await?;
        Ok(())
    }

    /// Return the agent to [`AgentState::Idle`].
    pub async fn stop(&self) {
        *self.state.write().await = AgentState::Idle;
    }

    /// Where the agent currently stands.
    pub async fn state(&self) -> AgentState {
        *self.state.read().await
    }

    /// Every message received so far.
    pub async fn get_inbox(&self) -> Vec<InboxEntry> {
        self.inbox.read().await.clone()
    }

    /// Every peer this agent can address.
    #[must_use]
    pub fn get_peers(&self) -> Vec<Peer> {
        self.config.peers.clone()
    }

    /// Received and in-flight messages, as JSON for the dashboard.
    ///
    /// Inbox entries win over pending ones with the same `msg_id`.
    pub async fn get_all_messages(&self) -> Vec<Value> {
        let inbox = self.inbox.read().await;
        let mut result: Vec<Value> = inbox
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "msg_id": entry.msg_id,
                    "corr_id": entry.corr_id,
                    "session_id": entry.session_id,
                    "run_id": entry.run_id,
                    "sender_agent": entry.sender_agent,
                    "sender_machine": entry.sender_machine,
                    "recipient_agent": entry.recipient_agent,
                    "recipient_machine": entry.recipient_machine,
                    "intent": entry.intent,
                    "status": entry.status,
                    "error": entry.error,
                    "payload": entry.payload,
                    "received_at": entry.received_at,
                })
            })
            .collect();

        let pending = self.pending.read().await;
        for (msg_id, pending_msg) in pending.iter() {
            if result
                .iter()
                .any(|m| m.get("msg_id").and_then(Value::as_str) == Some(msg_id.as_str()))
            {
                continue;
            }
            let env = &pending_msg.message.envelope;
            result.push(serde_json::json!({
                "msg_id": env.msg_id,
                "corr_id": env.corr_id,
                "session_id": env.session_id,
                "run_id": env.run_id,
                "sender_agent": env.sender.agent_id,
                "sender_machine": env.sender.machine_id,
                "recipient_agent": env.recipient.agent_id,
                "recipient_machine": env.recipient.machine_id,
                "intent": env.intent.as_str(),
                "status": pending_msg.state.as_str(),
                "error": env.error.as_deref().unwrap_or(""),
                "stream_id": pending_msg.stream_id,
                "received_at": pending_msg.created_at,
            }));
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn extract_bundle_from_payload(payload: Option<&Value>) -> Option<ContextBundle> {
    let bundle_val = payload?.get("bundle")?;
    serde_json::from_value(bundle_val.clone()).ok()
}

/// Overlay `additional` onto `original`.
///
/// Two JSON objects merge key-by-key; anything else is replaced outright, since
/// there is no meaningful way to overlay a scalar.
fn merge_payload(original: Option<&Value>, additional: Value) -> Value {
    match (original.and_then(Value::as_object), additional) {
        (Some(base), Value::Object(add)) => {
            let mut merged = base.clone();
            merged.extend(add);
            Value::Object(merged)
        }
        (_, add) => add,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_two_objects_keeps_untouched_keys() {
        let original = serde_json::json!({"a": 1, "b": 2});

        let merged = merge_payload(Some(&original), serde_json::json!({"b": 3}));

        assert_eq!(merged["a"], 1);
    }

    #[test]
    fn merging_two_objects_overwrites_shared_keys() {
        let original = serde_json::json!({"a": 1, "b": 2});

        let merged = merge_payload(Some(&original), serde_json::json!({"b": 3}));

        assert_eq!(merged["b"], 3);
    }

    #[test]
    fn merging_onto_a_non_object_replaces_it() {
        let original = serde_json::json!("scalar");

        let merged = merge_payload(Some(&original), serde_json::json!({"b": 3}));

        assert_eq!(merged, serde_json::json!({"b": 3}));
    }

    #[test]
    fn a_payload_without_a_bundle_yields_none() {
        let payload = serde_json::json!({"intent": "handoff"});

        assert!(extract_bundle_from_payload(Some(&payload)).is_none());
    }

    #[test]
    fn a_payload_with_a_bundle_yields_it() {
        let handoff = HandoffMessage::new(
            acp_core::chp::build_handoff("outcome", "stop", "T1", "desc", "agent-alpha"),
            HandoffIntent::Handoff,
        );
        let payload = serde_json::to_value(&handoff).unwrap();

        let bundle = extract_bundle_from_payload(Some(&payload)).unwrap();

        assert_eq!(bundle.active_work.task_id, "T1");
    }
}
