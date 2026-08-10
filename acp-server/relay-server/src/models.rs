//! ACP wire models
//!
//! Deliberately looser than `acp-core`'s: the relay forwards messages it does not
//! interpret, so `intent` and `priority` stay strings rather than enums and no
//! unknown value can make a message unforwardable.

use serde::{Deserialize, Serialize};

/// Routing metadata the relay reads and rewrites.
///
/// Absent optional fields are omitted rather than emitted as `null`: agents
/// deserialize `intent` and `priority` into non-optional enums, which accept a
/// missing key but reject an explicit null.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// Unique ID of this message.
    pub msg_id: String,
    /// Groups every message belonging to one exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corr_id: Option<String>,
    /// Who started the chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<AgentAddr>,
    /// Who sent this hop.
    pub sender: AgentAddr,
    /// Who this hop is addressed to.
    pub recipient: AgentAddr,
    /// Route a reply must unwind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<ReplyTo>,
    /// What action the message represents, as a wire string.
    #[serde(default = "default_intent")]
    pub intent: String,
    /// MIME type of the payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Scheduling hint, as a wire string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// RFC 3339 instant after which the message is stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    /// Failure description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Hop count and audit trail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hops: Option<Hops>,
}

fn default_intent() -> String {
    "delegate".to_string()
}

/// Agent address in `agent_id@machine_id` form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAddr {
    /// Logical agent name.
    pub agent_id: String,
    /// Host the agent runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
}

/// Reply routing path and stream endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyTo {
    /// Addresses to unwind on the way back, oldest hop first.
    #[serde(default)]
    pub path: Vec<String>,
    /// Where the originator accepts streamed replies.
    #[serde(default)]
    pub ws_endpoint: Option<String>,
}

/// Hop tracking — count, ceiling, and audit trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hops {
    /// Hops taken so far.
    pub count: u32,
    /// Hop ceiling.
    pub max: u32,
    /// One entry per hop, in order.
    #[serde(default)]
    pub trace: Vec<HopTraceEntry>,
}

/// One entry in [`Hops::trace`].
///
/// The protocol specifies `{agent_id, machine_id, timestamp}`, which is what
/// agents deserialize. Earlier relay builds wrote a bare `"agent_id@machine_id"`
/// string, so reading accepts both forms and normalizes to the structured one —
/// messages already persisted in the old shape stay readable.
#[derive(Debug, Clone, Serialize)]
pub struct HopTraceEntry {
    /// Agent that handled this hop.
    pub agent_id: String,
    /// Machine that agent ran on.
    pub machine_id: String,
    /// RFC 3339 timestamp of the hop.
    pub timestamp: String,
}

impl HopTraceEntry {
    /// A trace entry for `agent_id@machine_id`, stamped now.
    #[must_use]
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
            Raw::Structured {
                agent_id,
                machine_id,
                timestamp,
            } => Self {
                agent_id,
                machine_id,
                timestamp,
            },
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

/// One message the relay is holding for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMessage {
    /// Routing metadata.
    pub envelope: Envelope,
    /// Application data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// Body of `POST /acp/v1/messages/send`.
#[derive(Debug, Clone, Deserialize)]
pub struct SendRequest {
    /// Routing metadata.
    pub envelope: Envelope,
    /// Application data.
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

/// The relay's answer to a send.
#[derive(Debug, Clone, Serialize)]
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

/// A registered agent the relay can route to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    /// Logical agent name; the registry's primary key.
    pub agent_id: String,
    /// Host the agent runs on.
    pub machine_id: String,
    /// Base URL the relay pushes to.
    pub http_endpoint: String,
    /// URL the agent accepts streams on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_endpoint: Option<String>,
    /// What the agent advertises it can do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    /// Unix seconds of the last registration or poll.
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

/// Claims carried by a verified relay token.
#[derive(Debug, Clone)]
pub struct TokenClaims {
    /// Issuer, as `agent_id@machine_id`.
    pub iss: String,
    /// Audience, as `agent_id@machine_id`.
    pub sub: String,
    /// Message this token is bound to.
    pub msg_id: String,
    /// Expiry, as Unix seconds.
    pub exp: i64,
}
