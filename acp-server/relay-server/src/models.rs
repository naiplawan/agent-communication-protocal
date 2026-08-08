//! ACP data models

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub msg_id: String,
    #[serde(default)]
    pub corr_id: Option<String>,
    #[serde(default)]
    pub origin: Option<AgentAddr>,
    pub sender: AgentAddr,
    pub recipient: AgentAddr,
    #[serde(default)]
    pub reply_to: Option<ReplyTo>,
    #[serde(default = "default_intent")]
    pub intent: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub hops: Option<Hops>,
}

fn default_intent() -> String {
    "delegate".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAddr {
    pub agent_id: String,
    #[serde(default)]
    pub machine_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyTo {
    #[serde(default)]
    pub path: Vec<String>,
    #[serde(default)]
    pub ws_endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hops {
    pub count: u32,
    pub max: u32,
    #[serde(default)]
    pub trace: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMessage {
    pub envelope: Envelope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SendRequest {
    pub envelope: Envelope,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SendResponse {
    pub msg_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub agent_id: String,
    pub machine_id: String,
    pub http_endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct TokenClaims {
    pub iss: String,
    pub sub: String,
    pub msg_id: String,
    pub exp: i64,
}
