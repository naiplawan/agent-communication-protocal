//! ACP data models

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
// Absent optional fields are omitted rather than emitted as `null`: agents
// deserialize `intent` and `priority` into non-optional enums, which accept a
// missing key but reject an explicit null.
pub struct Envelope {
    pub msg_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corr_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<AgentAddr>,
    pub sender: AgentAddr,
    pub recipient: AgentAddr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<ReplyTo>,
    #[serde(default = "default_intent")]
    pub intent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hops: Option<Hops>,
}

fn default_intent() -> String {
    "delegate".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAddr {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    pub trace: Vec<HopTraceEntry>,
}

/// One entry in `hops.trace`.
///
/// The protocol specifies `{agent_id, machine_id, timestamp}`, which is what
/// agents deserialize. Earlier relay builds wrote a bare `"agent_id@machine_id"`
/// string, so reading accepts both forms and normalizes to the structured one —
/// messages already persisted in the old shape stay readable.
#[derive(Debug, Clone, Serialize)]
pub struct HopTraceEntry {
    pub agent_id: String,
    pub machine_id: String,
    pub timestamp: String,
}

impl HopTraceEntry {
    pub fn now(agent_id: &str, machine_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            machine_id: machine_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}

impl<'de> Deserialize<'de> for HopTraceEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Structured {
                agent_id: String,
                #[serde(default)]
                machine_id: String,
                #[serde(default)]
                timestamp: String,
            },
            Legacy(String),
        }

        Ok(match Raw::deserialize(deserializer)? {
            Raw::Structured { agent_id, machine_id, timestamp } => {
                Self { agent_id, machine_id, timestamp }
            }
            Raw::Legacy(s) => {
                let (agent_id, machine_id) = s.split_once('@').unwrap_or((s.as_str(), ""));
                Self {
                    agent_id: agent_id.to_string(),
                    machine_id: machine_id.to_string(),
                    timestamp: String::new(),
                }
            }
        })
    }
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
    /// False once a push to `http_endpoint` has failed. Poll-only agents live
    /// here permanently; it suppresses the forward attempt, not the peer.
    #[serde(default = "default_reachable")]
    pub reachable: bool,
}

fn default_reachable() -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct TokenClaims {
    pub iss: String,
    pub sub: String,
    pub msg_id: String,
    pub exp: i64,
}
