//! ACP Agent Signaling Client
//!
//! Handles NAT traversal by registering with a cloud relay and polling for messages.

use anyhow::Context;
use acp_core::protocol::Envelope;
use acp_core::security::create_token;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SignalingConfig {
    pub relay_url: String,        // e.g. "http://cloud-relay:8443"
    pub agent_id: String,
    pub machine_id: String,
    pub http_endpoint: String,    // public URL of THIS signaling server
    pub shared_secret: String,
    pub poll_interval_secs: u64,
}

impl SignalingConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            relay_url: std::env::var("ACP_RELAY_URL")
                .context("ACP_RELAY_URL must be set")?,
            agent_id: std::env::var("ACP_AGENT_ID")
                .context("ACP_AGENT_ID must be set")?,
            machine_id: std::env::var("ACP_MACHINE_ID")
                .unwrap_or_else(|_| hostname::get()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "local".to_string())),
            http_endpoint: std::env::var("ACP_HTTP_ENDPOINT")
                .unwrap_or_else(|_| format!("http://localhost:8444")),
            shared_secret: std::env::var("ACP_SHARED_SECRET")
                .context("ACP_SHARED_SECRET must be set")?,
            poll_interval_secs: std::env::var("ACP_POLL_INTERVAL")
                .unwrap_or_else(|_| "5".to_string())
                .parse()
                .unwrap_or(5),
        })
    }
}

// ---------------------------------------------------------------------------
// Relay client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMessage {
    pub envelope: Envelope,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendResponse {
    pub msg_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PendingResponse {
    messages: Vec<PendingMessage>,
}

pub struct SignalingClient {
    config: SignalingConfig,
    http_client: Client,
}

impl SignalingClient {
    pub fn new(config: SignalingConfig) -> Self {
        Self {
            config,
            http_client: Client::new(),
        }
    }

    fn make_token(&self, audience_agent: &str, audience_machine: &str, msg_id: &str) -> String {
        create_token(
            &self.config.agent_id,
            &self.config.machine_id,
            audience_agent,
            audience_machine,
            msg_id,
            &self.config.shared_secret,
            3600,
        )
    }

    /// Register this agent with the relay
    pub async fn register(&self) -> anyhow::Result<()> {
        let url = format!("{}/acp/v1/agents/register", self.config.relay_url);

        let body = serde_json::json!({
            "agent_id": self.config.agent_id,
            "machine_id": self.config.machine_id,
            "http_endpoint": self.config.http_endpoint,
            "capabilities": ["signaling", "agent"],
        });

        // The relay registers the peer named by this token's issuer, so it must be
        // signed as this agent — an unsigned registration is refused.
        let reg_id = format!("reg_{}", uuid::Uuid::new_v4());
        let token = self.make_token("acp-relay", "relay", &reg_id);

        let resp = self.http_client
            .post(&url)
            .json(&body)
            .header("Authorization", format!("ACP-Token {}", token))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Registration failed: {}", resp.status());
        }

        tracing::info!("[SIG] Registered with relay OK");
        Ok(())
    }

    /// Send a message via relay
    pub async fn send(&self, envelope: Envelope, payload: Option<serde_json::Value>) -> anyhow::Result<SendResponse> {
        let msg_id = &envelope.msg_id;
        let recipient_agent = &envelope.recipient.agent_id;
        let recipient_machine = envelope.recipient.machine_id.as_deref().unwrap_or("");

        let token = self.make_token(recipient_agent, recipient_machine, msg_id);
        let url = format!("{}/acp/v1/messages/send", self.config.relay_url);

        let resp = self.http_client
            .post(&url)
            .json(&serde_json::json!({ "envelope": envelope, "payload": payload }))
            .header("Authorization", format!("ACP-Token {}", token))
            .timeout(Duration::from_secs(15))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Send failed: {}", resp.text().await?);
        }

        let data: SendResponse = resp.json().await?;
        Ok(data)
    }

    /// Poll for pending messages from relay
    pub async fn poll_pending(&self) -> anyhow::Result<Vec<PendingMessage>> {
        let msg_id = format!("poll_{}", uuid::Uuid::new_v4());
        let token = self.make_token("acp-relay", "relay", &msg_id);
        let url = format!("{}/acp/v1/messages/pending", self.config.relay_url);

        let resp = self.http_client
            .get(&url)
            .header("Authorization", format!("ACP-Token {}", token))
            .timeout(Duration::from_secs(30))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Poll failed: {}", resp.status());
        }

        let data: PendingResponse = resp.json().await?;
        Ok(data.messages)
    }

    /// Acknowledge a message to relay
    pub async fn ack(&self, msg_id: &str) -> anyhow::Result<()> {
        let token = self.make_token("acp-relay", "relay", msg_id);
        let url = format!("{}/acp/v1/messages/{}/ack", self.config.relay_url, msg_id);

        let resp = self.http_client
            .post(&url)
            .json(&serde_json::json!({ "ack_type": "hop_ack", "received": true }))
            .header("Authorization", format!("ACP-Token {}", token))
            .timeout(Duration::from_secs(5))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Ack failed: {}", resp.status());
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

/// Re-register with relay every 4 minutes
pub async fn register_loop(config: SignalingConfig) {
    let client = SignalingClient::new(config.clone());

    loop {
        if let Err(e) = client.register().await {
            tracing::error!("[SIG] Registration failed: {}", e);
        }
        tokio::time::sleep(Duration::from_secs(240)).await;
    }
}

/// Poll relay for pending messages
pub async fn poll_loop(
    config: SignalingConfig,
    pending_store: Arc<RwLock<Vec<PendingMessage>>>,
) {
    let client = SignalingClient::new(config.clone());
    let mut interval = interval(Duration::from_secs(config.poll_interval_secs));

    loop {
        interval.tick().await;

        match client.poll_pending().await {
            Ok(msgs) if !msgs.is_empty() => {
                tracing::info!("[SIG] Got {} messages from relay", msgs.len());
                let mut pending = pending_store.write().await;
                pending.extend(msgs);
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("[SIG] Poll failed: {}", e);
            }
        }
    }
}
