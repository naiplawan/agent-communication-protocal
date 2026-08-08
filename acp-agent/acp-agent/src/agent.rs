//! ACP Agent — base agent class with send/receive/delegate, reply routing, and streaming
//!
//! Includes ACP-CHP: Context Handoff Protocol support.

use acp_core::config::{load_config, resolve_this_agent, ACPConfig, ThisAgent};
use acp_core::protocol::{
    self, build_envelope, forward_envelope, reply_envelope, new_msg_id, new_corr_id,
    new_stream_id, Envelope, Message, Origin, Intent, Priority, HopsExceededError,
    ReplyPathEmptyError, StreamFrame, build_ws_frame,
};
use acp_core::security::PeerAuth;
use acp_core::transport::ACPHttpClient;
use acp_core::chp::{
    ContextBundle, HandoffMessage, HandoffIntent, TaskStatus,
    build_handoff, build_progress,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Received,
    Forwarding,
    Processing,
    Replying,
    Complete,
    Failed,
}

impl Default for AgentState {
    fn default() -> Self {
        Self::Idle
    }
}

// ---------------------------------------------------------------------------
// Pending message tracking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PendingMessage {
    pub message: Message,
    pub state: AgentState,
    pub retry_count: u32,
    pub created_at: f64,
    pub stream_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Message handler type
// ---------------------------------------------------------------------------

pub type MessageHandler = Arc<dyn Fn(Message) -> HandlerResult + Send + Sync>;

pub type HandlerResult = Option<Value>;

// ---------------------------------------------------------------------------
// ACPAgent
// ---------------------------------------------------------------------------

pub struct ACPAgent {
    pub config: Arc<ACPConfig>,
    pub this: ThisAgent,
    pub client: Arc<ACPHttpClient>,
    handlers: Arc<RwLock<HashMap<String, MessageHandler>>>,
    handoff_handlers: Arc<RwLock<HashMap<String, MessageHandler>>>,
    pending: Arc<RwLock<HashMap<String, PendingMessage>>>,
    state: Arc<RwLock<AgentState>>,
    running: Arc<RwLock<bool>>,
}

impl ACPAgent {
    // ---- Construction ----

    /// Load config from file, environment, or default locations
    pub async fn from_config_file(config_path: Option<&str>) -> anyhow::Result<Self> {
        let config = load_config(config_path).map_err(|e| anyhow::anyhow!("{}", e))?;
        Self::from_config(config).await
    }

    /// Construct from a pre-loaded config
    pub async fn from_config(config: ACPConfig) -> anyhow::Result<Self> {
        let this = resolve_this_agent(&config).map_err(|e| anyhow::anyhow!("{}", e))?;

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
            state: Arc::new(RwLock::new(AgentState::Idle)),
            running: Arc::new(RwLock::new(false)),
        })
    }

    // ---- Handler registration ----

    /// Register a handler for DELEGATE intent messages
    pub fn on_delegate<F>(&self, f: F)
    where
        F: Fn(Message) -> HandlerResult + Send + Sync + 'static,
    {
        let handler: MessageHandler = Arc::new(move |msg| f(msg));
        // Use a simple string key for the intent
        let intent_str = Intent::Delegate.as_str().to_string();
        let mut h = self.handlers.blocking_write();
        h.insert(intent_str, handler);
    }

    /// Register a handler for REPLY intent messages
    pub fn on_reply<F>(&self, f: F)
    where
        F: Fn(Message) -> HandlerResult + Send + Sync + 'static,
    {
        let handler: MessageHandler = Arc::new(move |msg| f(msg));
        let intent_str = Intent::Reply.as_str().to_string();
        let mut h = self.handlers.blocking_write();
        h.insert(intent_str, handler);
    }

    /// Register a handler for ERROR intent messages
    pub fn on_error<F>(&self, f: F)
    where
        F: Fn(Message) -> HandlerResult + Send + Sync + 'static,
    {
        let handler: MessageHandler = Arc::new(move |msg| f(msg));
        let intent_str = Intent::Error.as_str().to_string();
        let mut h = self.handlers.blocking_write();
        h.insert(intent_str, handler);
    }

    /// Register a handler for CHP HANDOFF messages
    pub fn on_handoff<F>(&self, f: F)
    where
        F: Fn(ContextBundle, Message) -> HandlerResult + Send + Sync + 'static,
    {
        let handler: MessageHandler = Arc::new(move |msg| {
            let bundle = extract_bundle_from_payload(&msg.payload)?;
            f(bundle, msg)
        });
        let intent_str = HandoffIntent::Handoff.as_str().to_string();
        let mut h = self.handoff_handlers.blocking_write();
        h.insert(intent_str, handler);
    }

    /// Register a handler for CHP HANDOVER_REQUEST messages
    pub fn on_handover_request<F>(&self, f: F)
    where
        F: Fn(ContextBundle, Message) -> HandlerResult + Send + Sync + 'static,
    {
        let handler: MessageHandler = Arc::new(move |msg| {
            let bundle = extract_bundle_from_payload(&msg.payload)?;
            f(bundle, msg)
        });
        let intent_str = HandoffIntent::HandoverRequest.as_str().to_string();
        let mut h = self.handoff_handlers.blocking_write();
        h.insert(intent_str, handler);
    }

    // ---- Send operations ----

    /// Send a new message to a peer (originates from this agent or a human)
    pub async fn send_message(
        &self,
        target_agent_id: &str,
        payload: Value,
        origin: Option<Origin>,
    ) -> anyhow::Result<Message> {
        let peer = self.config.get_peer(target_agent_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown peer: {}", target_agent_id))?;

        let msg_id = new_msg_id();
        let corr_id = new_corr_id();

        let origin = origin.unwrap_or_else(|| Origin {
            agent_id: Some(self.this.agent_id.clone()),
            machine_id: self.this.machine_id.clone(),
            human_id: None,
        });

        let envelope = build_envelope(
            msg_id,
            corr_id,
            origin,
            &self.this.agent_id,
            self.this.machine_id.as_deref().unwrap_or(""),
            &peer.agent_id,
            &peer.machine_id,
            Intent::Delegate,
            Some(vec![format!(
                "{}@{}",
                self.this.agent_id,
                self.this.machine_id.as_deref().unwrap_or("")
            )]),
            self.this.ws_endpoint.clone(),
            10,
            "application/json",
            Priority::Normal,
            None,
        );

        let message = Message::with_payload(envelope, payload.clone());
        self.client
            .send_with_retry(peer, &message)
            .await
            .map_err(|e| anyhow::anyhow!("Send failed: {}", e))?;

        Ok(message)
    }

    /// Send a CHP handoff message to another agent
    pub async fn send_handoff(
        &self,
        target_agent_id: &str,
        bundle: ContextBundle,
        origin: Option<Origin>,
    ) -> anyhow::Result<Message> {
        let handoff_msg = HandoffMessage::new(bundle, HandoffIntent::Handoff);

        let payload = serde_json::to_value(&handoff_msg)?;

        self.send_message(target_agent_id, payload, origin).await
    }

    /// Forward an incoming message to another agent (delegate)
    pub async fn delegate_to(
        &self,
        target_agent_id: &str,
        original_message: &Message,
        additional_payload: Option<Value>,
    ) -> anyhow::Result<Message> {
        let peer = self.config.get_peer(target_agent_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown peer: {}", target_agent_id))?;

        let new_envelope = forward_envelope(
            &original_message.envelope,
            &self.this.agent_id,
            self.this.machine_id.as_deref().unwrap_or(""),
            &peer.agent_id,
            &peer.machine_id,
        )
        .map_err(|e| anyhow::anyhow!("{}", e))?;

        let merged_payload = if let Some(add) = additional_payload {
            merge_payload(original_message.payload.clone(), add)
        } else {
            original_message.payload.clone().unwrap_or(serde_json::Value::Null)
        };

        let new_message = Message::with_payload(new_envelope, merged_payload);

        {
            let mut pending = self.pending.write().await;
            pending.insert(
                original_message.envelope.msg_id.clone(),
                PendingMessage {
                    message: new_message.clone(),
                    state: AgentState::Forwarding,
                    retry_count: 0,
                    created_at: now_f64(),
                    stream_id: None,
                },
            );
        }

        self.client
            .send_with_retry(peer, &new_message)
            .await
            .map_err(|e| anyhow::anyhow!("Delegate failed: {}", e))?;

        Ok(new_message)
    }

    /// Send a reply back along reply_to.path
    pub async fn reply_to(
        &self,
        original_message: &Message,
        payload: Value,
    ) -> anyhow::Result<Message> {
        let new_envelope = reply_envelope(
            &original_message.envelope,
            &self.this.agent_id,
            self.this.machine_id.as_deref().unwrap_or(""),
        )
        .map_err(|e| anyhow::anyhow!("{}", e))?;

        let recipient_addr = format!(
            "{}@{}",
            new_envelope.recipient.agent_id,
            new_envelope.recipient.machine_id.as_deref().unwrap_or("")
        );

        let peer = self.config.get_peer_by_addr(&recipient_addr)
            .ok_or_else(|| anyhow::anyhow!("Peer not found for reply recipient: {}", recipient_addr))?;

        let new_message = Message::with_payload(new_envelope, payload);

        {
            let mut pending = self.pending.write().await;
            pending.insert(
                original_message.envelope.msg_id.clone(),
                PendingMessage {
                    message: new_message.clone(),
                    state: AgentState::Replying,
                    retry_count: 0,
                    created_at: now_f64(),
                    stream_id: None,
                },
            );
        }

        self.client
            .send_with_retry(peer, &new_message)
            .await
            .map_err(|e| anyhow::anyhow!("Reply failed: {}", e))?;

        Ok(new_message)
    }

    /// Propagate an error back along the reply path
    pub async fn send_error(
        &self,
        original_message: &Message,
        error: &str,
        retryable: bool,
    ) -> anyhow::Result<Message> {
        let mut new_envelope = reply_envelope(
            &original_message.envelope,
            &self.this.agent_id,
            self.this.machine_id.as_deref().unwrap_or(""),
        )
        .map_err(|e| anyhow::anyhow!("{}", e))?;

        new_envelope.error = Some(error.to_string());
        new_envelope.intent = Intent::Error;

        let recipient_addr = format!(
            "{}@{}",
            new_envelope.recipient.agent_id,
            new_envelope.recipient.machine_id.as_deref().unwrap_or("")
        );

        let peer = self.config.get_peer_by_addr(&recipient_addr)
            .ok_or_else(|| anyhow::anyhow!("Peer not found for error recipient: {}", recipient_addr))?;

        let error_payload = serde_json::json!({
            "error": error,
            "retryable": retryable,
            "failed_at": format!("{}@{}", self.this.agent_id, self.this.machine_id.as_deref().unwrap_or("")),
        });

        let new_message = Message::with_payload(new_envelope, error_payload);

        self.client
            .send_with_retry(peer, &new_message)
            .await
            .map_err(|e| anyhow::anyhow!("Error send failed: {}", e))?;

        Ok(new_message)
    }

    // ---- Streaming ----

    /// Initiate a stream for a reply
    pub async fn initiate_stream(&self, original_message: &Message) -> String {
        let stream_id = new_stream_id();
        let mut pending = self.pending.write().await;
        let entry = pending.entry(original_message.envelope.msg_id.clone()).or_insert(PendingMessage {
            message: original_message.clone(),
            state: AgentState::Processing,
            retry_count: 0,
            created_at: now_f64(),
            stream_id: None,
        });
        entry.stream_id = Some(stream_id.clone());
        stream_id
    }

    /// Send a stream chunk back toward origin
    pub async fn send_stream_chunk(
        &self,
        original_message: &Message,
        stream_id: &str,
        seq: u32,
        total: Option<u32>,
        data: Value,
        final_: bool,
    ) -> anyhow::Result<()> {
        let path = original_message.envelope.reply_to.as_ref()
            .map(|rt| rt.path.clone())
            .unwrap_or_default();

        let Some(next_hop_addr) = path.first().cloned() else {
            tracing::warn!("No reply_to.path for stream {}", stream_id);
            return Ok(());
        };

        let peer = self.config.get_peer_by_addr(&next_hop_addr)
            .ok_or_else(|| anyhow::anyhow!("No peer for {}", next_hop_addr))?;

        let chunk_envelope = reply_envelope(
            &original_message.envelope,
            &self.this.agent_id,
            self.this.machine_id.as_deref().unwrap_or(""),
        )
        .map_err(|e| anyhow::anyhow!("{}", e))?;

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

        self.client
            .send_with_retry(peer, &chunk_message)
            .await
            .map_err(|e| anyhow::anyhow!("Stream chunk send failed: {}", e))?;

        Ok(())
    }

    /// Send a reply as a stream of chunks
    pub async fn stream_reply(
        &self,
        original_message: &Message,
        chunks: Vec<(Value, bool)>,
    ) -> anyhow::Result<Message> {
        let stream_id = self.initiate_stream(original_message).await;
        let total = chunks.len() as u32;

        for (seq, (data, final_)) in chunks.into_iter().enumerate() {
            self.send_stream_chunk(
                original_message,
                &stream_id,
                seq as u32,
                Some(total),
                data,
                final_,
            )
            .await?;
        }

        let summary_payload = serde_json::json!({
            "stream_id": stream_id,
            "total_frames": total,
            "origin": format!("{}@{}", self.this.agent_id, self.this.machine_id.as_deref().unwrap_or("")),
        });

        self.reply_to(original_message, summary_payload).await
    }

    // ---- Message processing ----

    /// Process a received message
    pub async fn process_message(&self, message: Message) -> HandlerResult {
        *self.state.write().await = AgentState::Processing;

        // Check if this is a CHP message
        if let Some(ref payload) = message.payload {
            if let Some(intent) = payload.get("intent").and_then(|v| v.as_str()) {
                let handoff_handlers = self.handoff_handlers.read().await;
                if let Some(handler) = handoff_handlers.get(intent) {
                    return handler(message);
                }
            }
        }

        // Standard ACP handler
        let intent_str = message.envelope.intent.as_str().to_string();
        let handlers = self.handlers.read().await;

        if let Some(handler) = handlers.get(&intent_str) {
            handler(message)
        } else {
            tracing::warn!("No handler for intent: {}", intent_str);
            None
        }
    }

    /// Handle an incoming message — process and auto-reply if handler returns result
    pub async fn handle_incoming(&self, message: Message) {
        let msg_id = message.envelope.msg_id.clone();

        {
            let mut pending = self.pending.write().await;
            pending.insert(
                msg_id.clone(),
                PendingMessage {
                    message: message.clone(),
                    state: AgentState::Received,
                    retry_count: 0,
                    created_at: now_f64(),
                    stream_id: None,
                },
            );
        }

        *self.state.write().await = AgentState::Received;

        let result = self.process_message(message.clone()).await;

        if let Some(result_payload) = result {
            if message.envelope.reply_to.is_some() {
                if let Err(e) = self.reply_to(&message, result_payload).await {
                    tracing::warn!("Auto-reply failed: {}", e);
                }
            }
        }

        *self.state.write().await = AgentState::Complete;
    }

    // ---- Server lifecycle ----

    /// Start the HTTP server (blocking)
    pub async fn run(&self) -> anyhow::Result<()> {
        *self.running.write().await = true;
        *self.state.write().await = AgentState::Idle;

        let addr = self.this.http_endpoint.as_ref()
            .and_then(|ep| ep.split(':').nth(1))
            .unwrap_or("8443");

        tracing::info!("[{}] ACP Agent starting on port {}", self.this.agent_id, addr);

        // TODO: integrate with server module
        // For now, just keep running
        tokio::signal::ctrl_c().await?;
        *self.running.write().await = false;
        Ok(())
    }

    /// Stop the agent
    pub async fn stop(&self) {
        *self.running.write().await = false;
        *self.state.write().await = AgentState::Idle;
    }

    /// Get current state
    pub async fn state(&self) -> AgentState {
        self.state.read().await.clone()
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

fn extract_bundle_from_payload(payload: &Option<Value>) -> Option<ContextBundle> {
    let payload = payload.as_ref()?;
    let bundle_val = payload.get("bundle")?;
    let bundle: ContextBundle = serde_json::from_value(bundle_val.clone()).ok()?;
    Some(bundle)
}

fn merge_payload(original: Option<Value>, additional: Value) -> Value {
    match (original, additional) {
        (Some(obj), add_obj) if obj.is_object() && add_obj.is_object() => {
            let mut merged = obj.as_object().unwrap().clone();
            for (k, v) in add_obj.as_object().unwrap() {
                merged.insert(k.clone(), v.clone());
            }
            Value::Object(merged)
        }
        (_, add) => add,
    }
}

// ---------------------------------------------------------------------------
// Default handlers
// ---------------------------------------------------------------------------

impl ACPAgent {
    fn default_delegate_handler(msg: Message) -> HandlerResult {
        tracing::info!(
            "[agent] Received delegate: {} from {}",
            msg.envelope.msg_id,
            msg.envelope.sender.agent_id
        );
        None
    }

    fn default_reply_handler(msg: Message) -> HandlerResult {
        tracing::info!(
            "[agent] Received reply: {} from {}",
            msg.envelope.msg_id,
            msg.envelope.sender.agent_id
        );
        None
    }

    fn default_error_handler(msg: Message) -> HandlerResult {
        let err = msg.envelope.error.as_deref().unwrap_or("unknown");
        tracing::error!(
            "[agent] Error from {}: {}",
            msg.envelope.sender.agent_id,
            err
        );
        None
    }
}
