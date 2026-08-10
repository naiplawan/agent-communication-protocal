//! ACP Agent Signaling Client
//!
//! Handles NAT traversal by registering with a cloud relay and polling it for
//! messages, so an agent behind NAT never has to accept an inbound connection.

use std::sync::Arc;
use std::time::Duration;

use acp_core::protocol::Envelope;
use acp_core::security::create_token;
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::time::interval;

use crate::error::SignalingError;

/// The relay's agent ID, used as the audience of relay-addressed tokens.
const RELAY_AGENT_ID: &str = "acp-relay";

/// The relay's machine ID, likewise.
const RELAY_MACHINE_ID: &str = "relay";

/// Lifetime of tokens minted for the relay.
const TOKEN_TTL_SECONDS: i64 = 3600;

/// How often the agent re-announces itself, well inside the relay's peer TTL.
const REGISTER_INTERVAL: Duration = Duration::from_secs(240);

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Everything needed to talk to a relay, read from the environment.
#[derive(Clone)]
pub struct SignalingConfig {
    /// Base URL of the relay, e.g. `http://cloud-relay:8443`.
    pub relay_url: String,
    /// This agent's ID, which the relay registers the endpoint under.
    pub agent_id: String,
    /// Host this agent runs on.
    pub machine_id: String,
    /// Publicly reachable URL of this agent's own ACP server.
    pub http_endpoint: String,
    /// Secret shared with the relay, used to sign tokens.
    pub shared_secret: String,
    /// Seconds between polls for pending messages.
    pub poll_interval_secs: u64,
}

impl SignalingConfig {
    /// Read the config from the environment.
    ///
    /// `ACP_MACHINE_ID`, `ACP_HTTP_ENDPOINT`, and `ACP_POLL_INTERVAL` fall back
    /// to the hostname, `http://localhost:8444`, and 5 seconds.
    ///
    /// # Errors
    /// Returns [`SignalingError::MissingEnv`] when `ACP_RELAY_URL`,
    /// `ACP_AGENT_ID`, or `ACP_SHARED_SECRET` is unset.
    pub fn from_env() -> Result<Self, SignalingError> {
        Ok(Self {
            relay_url: required_env("ACP_RELAY_URL")?,
            agent_id: required_env("ACP_AGENT_ID")?,
            shared_secret: required_env("ACP_SHARED_SECRET")?,
            machine_id: std::env::var("ACP_MACHINE_ID").unwrap_or_else(|_| {
                hostname::get().map_or_else(
                    |_| "local".to_string(),
                    |name| name.to_string_lossy().to_string(),
                )
            }),
            http_endpoint: std::env::var("ACP_HTTP_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:8444".to_string()),
            poll_interval_secs: std::env::var("ACP_POLL_INTERVAL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
        })
    }
}

fn required_env(name: &'static str) -> Result<String, SignalingError> {
    std::env::var(name).map_err(|_| SignalingError::MissingEnv(name))
}

// ---------------------------------------------------------------------------
// Relay client
// ---------------------------------------------------------------------------

/// One message the relay is holding for this agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMessage {
    /// Routing metadata.
    pub envelope: Envelope,
    /// Application data.
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

/// The relay's answer to a send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendResponse {
    /// Message the response is about.
    pub msg_id: String,
    /// What the relay did with it — `"forwarded"`, `"brokered"`, `"accepted"`.
    pub status: String,
    /// Endpoint it was forwarded to, when it was forwarded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,
    /// Why it was refused, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PendingResponse {
    messages: Vec<PendingMessage>,
}

/// Authenticated client for one relay.
pub struct SignalingClient {
    config: SignalingConfig,
    http_client: Client,
}

impl SignalingClient {
    /// Build a client for the relay named by `config`.
    #[must_use]
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
            TOKEN_TTL_SECONDS,
        )
    }

    fn relay_token(&self, msg_id: &str) -> String {
        self.make_token(RELAY_AGENT_ID, RELAY_MACHINE_ID, msg_id)
    }

    /// Announce this agent's endpoint to the relay.
    ///
    /// # Errors
    /// Returns [`SignalingError::Request`] when the relay is unreachable, or
    /// [`SignalingError::Rejected`] when it refuses the registration.
    pub async fn register(&self) -> Result<(), SignalingError> {
        let body = serde_json::json!({
            "agent_id": self.config.agent_id,
            "machine_id": self.config.machine_id,
            "http_endpoint": self.config.http_endpoint,
            "capabilities": ["signaling", "agent"],
        });

        // The relay registers the peer named by this token's issuer, so it must be
        // signed as this agent — an unsigned registration is refused.
        let reg_id = format!("reg_{}", uuid::Uuid::new_v4());

        let resp = self
            .http_client
            .post(format!("{}/acp/v1/agents/register", self.config.relay_url))
            .json(&body)
            .header("Authorization", self.auth_header(&reg_id))
            .send()
            .await?;

        check_status(resp, "registration").await?;
        tracing::info!("[SIG] Registered with relay OK");
        Ok(())
    }

    /// Send a message through the relay.
    ///
    /// # Errors
    /// Returns [`SignalingError::Request`] when the relay is unreachable or the
    /// response is not a [`SendResponse`], or [`SignalingError::Rejected`] when
    /// the relay refuses the message.
    pub async fn send(
        &self,
        envelope: Envelope,
        payload: Option<serde_json::Value>,
    ) -> Result<SendResponse, SignalingError> {
        // Addressed to the ultimate recipient: the relay is a forwarder on this
        // path, not the target.
        let token = self.make_token(
            &envelope.recipient.agent_id,
            envelope.recipient.machine_id.as_deref().unwrap_or(""),
            &envelope.msg_id,
        );

        let resp = self
            .http_client
            .post(format!("{}/acp/v1/messages/send", self.config.relay_url))
            .json(&serde_json::json!({ "envelope": envelope, "payload": payload }))
            .header("Authorization", format!("ACP-Token {token}"))
            .timeout(Duration::from_secs(15))
            .send()
            .await?;

        Ok(check_status(resp, "send").await?.json().await?)
    }

    /// Collect the messages the relay is holding for this agent.
    ///
    /// # Errors
    /// Returns [`SignalingError::Request`] when the relay is unreachable, or
    /// [`SignalingError::Rejected`] when it refuses the poll.
    pub async fn poll_pending(&self) -> Result<Vec<PendingMessage>, SignalingError> {
        let poll_id = format!("poll_{}", uuid::Uuid::new_v4());

        let resp = self
            .http_client
            .get(format!("{}/acp/v1/messages/pending", self.config.relay_url))
            .header("Authorization", self.auth_header(&poll_id))
            .timeout(Duration::from_secs(30))
            .send()
            .await?;

        let data: PendingResponse = check_status(resp, "poll").await?.json().await?;
        Ok(data.messages)
    }

    /// Tell the relay a message was received, so it stops offering it.
    ///
    /// # Errors
    /// Returns [`SignalingError::Request`] when the relay is unreachable, or
    /// [`SignalingError::Rejected`] when it refuses the acknowledgement.
    pub async fn ack(&self, msg_id: &str) -> Result<(), SignalingError> {
        let resp = self
            .http_client
            .post(format!(
                "{}/acp/v1/messages/{msg_id}/ack",
                self.config.relay_url
            ))
            .json(&serde_json::json!({ "ack_type": "hop_ack", "received": true }))
            .header("Authorization", self.auth_header(msg_id))
            .timeout(Duration::from_secs(5))
            .send()
            .await?;

        check_status(resp, "ack").await?;
        Ok(())
    }

    fn auth_header(&self, msg_id: &str) -> String {
        format!("ACP-Token {}", self.relay_token(msg_id))
    }
}

async fn check_status(resp: Response, operation: &'static str) -> Result<Response, SignalingError> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status().as_u16();
    Err(SignalingError::Rejected {
        operation,
        status,
        body: resp.text().await.unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

/// Re-announce this agent to the relay every four minutes, forever.
pub async fn register_loop(config: SignalingConfig) {
    let client = SignalingClient::new(config);

    loop {
        if let Err(e) = client.register().await {
            tracing::error!("[SIG] Registration failed: {e}");
        }
        tokio::time::sleep(REGISTER_INTERVAL).await;
    }
}

/// Poll the relay for pending messages and append them to `pending_store`, forever.
pub async fn poll_loop(config: SignalingConfig, pending_store: Arc<RwLock<Vec<PendingMessage>>>) {
    let mut ticker = interval(Duration::from_secs(config.poll_interval_secs));
    let client = SignalingClient::new(config);

    loop {
        ticker.tick().await;

        match client.poll_pending().await {
            Ok(msgs) if !msgs.is_empty() => {
                tracing::info!("[SIG] Got {} messages from relay", msgs.len());
                pending_store.write().await.extend(msgs);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("[SIG] Poll failed: {e}"),
        }
    }
}
