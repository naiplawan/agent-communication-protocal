//! ACP Protocol — message envelope, ULID generation, path building, streaming frames

use serde::{Deserialize, Serialize};
use std::fmt;
use chrono::Utc;

// ---------------------------------------------------------------------------
// ID Generation
// ---------------------------------------------------------------------------

/// Generate a new message ID with "msg_" prefix
pub fn new_msg_id() -> String {
    format!("msg_{}", ulid::Ulid::new().to_string().to_lowercase())
}

/// Generate a new correlation ID
pub fn new_corr_id() -> String {
    new_msg_id()
}

/// Generate a new acknowledgement ID with "ack_" prefix
pub fn new_ack_id() -> String {
    format!("ack_{}", ulid::Ulid::new().to_string().to_lowercase())
}

/// Generate a new stream ID with "str_" prefix
pub fn new_stream_id() -> String {
    format!("str_{}", ulid::Ulid::new().to_string().to_lowercase())
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// ACP message intent — what action the message represents
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Delegate,
    Reply,
    Ack,
    Error,
    StreamStart,
    StreamChunk,
    StreamEnd,
    // ACP-CHP intents
    Handshake,
    HandshakeAck,
    HandshakeDecline,
    HandshakeQuery,
    Handoff,
    HandoverRequest,
    HandoverAccept,
    HandoverDecline,
    HandoverQuery,
    Progress,
    Blocked,
    Complete,
}

impl Intent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Intent::Delegate => "delegate",
            Intent::Reply => "reply",
            Intent::Ack => "ack",
            Intent::Error => "error",
            Intent::StreamStart => "stream_start",
            Intent::StreamChunk => "stream_chunk",
            Intent::StreamEnd => "stream_end",
            Intent::Handshake => "handshake",
            Intent::HandshakeAck => "handshake_ack",
            Intent::HandshakeDecline => "handshake_decline",
            Intent::HandshakeQuery => "handshake_query",
            Intent::Handoff => "handoff",
            Intent::HandoverRequest => "handover_request",
            Intent::HandoverAccept => "handover_accept",
            Intent::HandoverDecline => "handover_decline",
            Intent::HandoverQuery => "handover_query",
            Intent::Progress => "progress",
            Intent::Blocked => "blocked",
            Intent::Complete => "complete",
        }
    }
}

impl Default for Intent {
    fn default() -> Self {
        Intent::Delegate
    }
}

impl fmt::Display for Intent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Message priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Normal,
    High,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Normal => "normal",
            Priority::High => "high",
        }
    }
}

// ---------------------------------------------------------------------------
// Core dataclasses
// ---------------------------------------------------------------------------

/// Origin — who initiated the message chain
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Origin {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub human_id: Option<String>,
}

impl Origin {
    pub fn new() -> Self {
        Self {
            agent_id: None,
            machine_id: None,
            human_id: None,
        }
    }

    pub fn label(&self) -> String {
        if let Some(ref human_id) = self.human_id {
            human_id.clone()
        } else if let (Some(ref agent_id), Some(ref machine_id)) = (&self.agent_id, &self.machine_id) {
            format!("{}@{}", agent_id, machine_id)
        } else if let Some(ref agent_id) = self.agent_id {
            agent_id.clone()
        } else {
            "unknown".to_string()
        }
    }
}

impl Default for Origin {
    fn default() -> Self {
        Self::new()
    }
}

/// Agent address — agent_id@machine_id format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAddr {
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
}

impl AgentAddr {
    pub fn new(agent_id: impl Into<String>, machine_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            machine_id: Some(machine_id.into()),
        }
    }

    pub fn to_str(&self) -> String {
        match &self.machine_id {
            Some(m) => format!("{}@{}", self.agent_id, m),
            None => self.agent_id.clone(),
        }
    }

    pub fn from_str(s: &str) -> Self {
        if let Some((agent_id, machine_id)) = s.rsplit_once('@') {
            Self {
                agent_id: agent_id.to_string(),
                machine_id: Some(machine_id.to_string()),
            }
        } else {
            Self {
                agent_id: s.to_string(),
                machine_id: None,
            }
        }
    }
}

impl fmt::Display for AgentAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_str())
    }
}

/// Reply routing path and WebSocket endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReplyTo {
    #[serde(default)]
    pub path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_endpoint: Option<String>,
}

impl ReplyTo {
    pub fn new() -> Self {
        Self {
            path: Vec::new(),
            ws_endpoint: None,
        }
    }

    /// Append current sender to path when forwarding (forward direction)
    pub fn add_hop(&mut self, agent_id: &str, machine_id: &str) {
        self.path.push(format!("{}@{}", agent_id, machine_id));
    }

    /// Remove and return the next recipient from the path (reply direction)
    /// Pops from the BACK — the last sender is who just forwarded to this agent
    pub fn pop_next(&mut self) -> Option<String> {
        self.path.pop()
    }
}

impl Default for ReplyTo {
    fn default() -> Self {
        Self::new()
    }
}

/// Hop tracking — count, TTL, and audit trace
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Hops {
    pub count: u32,
    pub max: u32,
    #[serde(default)]
    pub trace: Vec<HopTraceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HopTraceEntry {
    pub agent_id: String,
    pub machine_id: String,
    pub timestamp: String,
}

impl Hops {
    pub fn new(max: u32) -> Self {
        Self {
            count: 0,
            max,
            trace: Vec::new(),
        }
    }

    /// Increment hop count, add trace entry. Returns false if max exceeded.
    pub fn increment(&mut self, agent_id: &str, machine_id: &str) -> bool {
        self.count += 1;
        self.trace.push(HopTraceEntry {
            agent_id: agent_id.to_string(),
            machine_id: machine_id.to_string(),
            timestamp: iso_now(),
        });
        self.count <= self.max
    }
}

impl Default for Hops {
    fn default() -> Self {
        Self::new(10)
    }
}

/// The envelope — the core ACP message metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Envelope {
    pub msg_id: String,
    #[serde(default)]
    pub corr_id: Option<String>,
    #[serde(default)]
    pub origin: Option<Origin>,
    pub sender: AgentAddr,
    pub recipient: AgentAddr,
    #[serde(default)]
    pub reply_to: Option<ReplyTo>,
    #[serde(default = "Intent::default")]
    pub intent: Intent,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default = "Priority::default")]
    pub priority: Priority,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub hops: Option<Hops>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

fn default_content_type() -> String {
    "application/json".to_string()
}

/// A full ACP message — envelope + payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub envelope: Envelope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

impl Message {
    pub fn new(envelope: Envelope) -> Self {
        Self { envelope, payload: None }
    }

    pub fn with_payload(envelope: Envelope, payload: serde_json::Value) -> Self {
        Self {
            envelope,
            payload: Some(payload),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap()
    }

    pub fn from_json(raw: &str) -> serde_json::Result<Self> {
        serde_json::from_str(raw)
    }
}

// ---------------------------------------------------------------------------
// Stream frames
// ---------------------------------------------------------------------------

/// WebSocket streaming frame
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StreamFrame {
    pub stream_id: String,
    pub msg_id: String,
    #[serde(default)]
    pub corr_id: Option<String>,
    pub seq: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
    pub final_: bool,
    #[serde(default = "iso_now")]
    pub timestamp: String,
}

fn iso_now() -> String {
    Utc::now().to_rfc3339()
}

/// Build a WebSocket frame for streaming
pub fn build_ws_frame(
    stream_id: &str,
    msg_id: &str,
    corr_id: &str,
    seq: u32,
    total: Option<u32>,
    final_: bool,
    data: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "frame": StreamFrame {
            stream_id: stream_id.to_string(),
            msg_id: msg_id.to_string(),
            corr_id: Some(corr_id.to_string()),
            seq,
            total,
            final_,
            timestamp: iso_now(),
        },
        "data": data
    })
}

/// Parse a WebSocket frame from a JSON value
pub fn parse_ws_frame(raw: &serde_json::Value) -> Option<StreamFrame> {
    let frame = raw.get("frame")?;
    Some(StreamFrame {
        stream_id: frame.get("stream_id")?.as_str()?.to_string(),
        msg_id: frame.get("msg_id")?.as_str()?.to_string(),
        corr_id: frame.get("corr_id").and_then(|v| v.as_str()).map(String::from),
        seq: frame.get("seq")?.as_u64()? as u32,
        total: frame.get("total").and_then(|v| v.as_u64()).map(|v| v as u32),
        final_: frame.get("final_").and_then(|v| v.as_bool()).unwrap_or(false),
        timestamp: frame
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or(&iso_now())
            .to_string(),
    })
}

// ---------------------------------------------------------------------------
// Envelope builders
// ---------------------------------------------------------------------------

/// Build a new envelope for a fresh message (not a forward)
#[must_use]
pub fn build_envelope(
    msg_id: String,
    corr_id: String,
    origin: Origin,
    sender_agent_id: &str,
    sender_machine_id: &str,
    recipient_agent_id: &str,
    recipient_machine_id: &str,
    intent: Intent,
    reply_to_path: Option<Vec<String>>,
    reply_to_ws_endpoint: Option<String>,
    hops_max: u32,
    content_type: &str,
    priority: Priority,
    deadline: Option<String>,
) -> Envelope {
    let reply_to = reply_to_path.map(|path| ReplyTo {
        path,
        ws_endpoint: reply_to_ws_endpoint,
    });

    Envelope {
        msg_id,
        corr_id: Some(corr_id),
        origin: Some(origin),
        sender: AgentAddr::new(sender_agent_id, sender_machine_id),
        recipient: AgentAddr::new(recipient_agent_id, recipient_machine_id),
        reply_to,
        intent,
        content_type: Some(content_type.to_string()),
        priority,
        deadline,
        error: None,
        hops: Some(Hops::new(hops_max)),
        extra: std::collections::HashMap::new(),
    }
}

/// Clone an envelope for forwarding to the next hop (delegation direction)
/// - Increments hop count
/// - Appends current sender to reply_to.path
/// - Updates sender/recipient
pub fn forward_envelope(
    envelope: &Envelope,
    new_sender_agent_id: &str,
    new_sender_machine_id: &str,
    new_recipient_agent_id: &str,
    new_recipient_machine_id: &str,
) -> Result<Envelope, HopsExceededError> {
    let mut hops = envelope.hops.clone().unwrap_or_else(|| Hops::new(10));
    if !hops.increment(new_sender_agent_id, new_sender_machine_id) {
        return Err(HopsExceededError {
            max: hops.max,
            at: format!("{}@{}", new_sender_agent_id, new_sender_machine_id),
        });
    }

    let mut new_reply_to = envelope.reply_to.clone().unwrap_or_default();
    new_reply_to.add_hop(new_sender_agent_id, new_sender_machine_id);

    let mut new_envelope = Envelope {
        msg_id: envelope.msg_id.clone(),
        corr_id: envelope.corr_id.clone(),
        origin: envelope.origin.clone(),
        sender: AgentAddr::new(new_sender_agent_id, new_sender_machine_id),
        recipient: AgentAddr::new(new_recipient_agent_id, new_recipient_machine_id),
        reply_to: Some(new_reply_to),
        intent: envelope.intent,
        content_type: envelope.content_type.clone(),
        priority: envelope.priority,
        deadline: envelope.deadline.clone(),
        error: envelope.error.clone(),
        hops: Some(hops),
        extra: envelope.extra.clone(),
    };

    Ok(new_envelope)
}

/// Clone an envelope for sending a reply back along the reply_to.path
/// - Pops LAST entry from reply_to.path (the agent who just forwarded to you)
/// - Changes intent to Reply
pub fn reply_envelope(
    envelope: &Envelope,
    new_sender_agent_id: &str,
    new_sender_machine_id: &str,
) -> Result<Envelope, ReplyPathEmptyError> {
    let mut reply_to = envelope.reply_to.clone().unwrap_or_default();
    let next_hop = reply_to
        .pop_next()
        .ok_or_else(|| ReplyPathEmptyError(envelope.msg_id.clone()))?;

    let (next_agent_id, next_machine_id) =
        next_hop.rsplit_once('@').map(|(a, m)| (a, m)).unwrap_or((&next_hop, ""));

    let mut hops = envelope.hops.clone().unwrap_or_else(|| Hops::new(10));
    if !hops.increment(new_sender_agent_id, new_sender_machine_id) {
        return Err(HopsExceededError {
            max: hops.max,
            at: format!("{}@{}", new_sender_agent_id, new_sender_machine_id),
        }
        .into());
    }

    Ok(Envelope {
        msg_id: envelope.msg_id.clone(),
        corr_id: envelope.corr_id.clone(),
        origin: envelope.origin.clone(),
        sender: AgentAddr::new(new_sender_agent_id, new_sender_machine_id),
        recipient: AgentAddr::new(next_agent_id, next_machine_id),
        reply_to: Some(reply_to),
        intent: Intent::Reply,
        content_type: envelope.content_type.clone(),
        priority: envelope.priority,
        deadline: envelope.deadline.clone(),
        error: envelope.error.clone(),
        hops: Some(hops),
        extra: envelope.extra.clone(),
    })
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct HopsExceededError {
    pub max: u32,
    pub at: String,
}

impl fmt::Display for HopsExceededError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Max hops ({}) exceeded at {}",
            self.max, self.at
        )
    }
}

impl std::error::Error for HopsExceededError {}

#[derive(Debug)]
pub struct ReplyPathEmptyError(pub String);

impl fmt::Display for ReplyPathEmptyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "No reply_to.path to route reply for {}", self.0)
    }
}

impl std::error::Error for ReplyPathEmptyError {}

impl From<HopsExceededError> for ReplyPathEmptyError {
    fn from(_: HopsExceededError) -> Self {
        ReplyPathEmptyError("hops exceeded".to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_msg_id_format() {
        let id = new_msg_id();
        assert!(id.starts_with("msg_"));
        assert_eq!(id.len(), 4 + 26); // "msg_" + 26 char ULID
    }

    #[test]
    fn test_agent_addr_to_from_str() {
        let addr = AgentAddr::new("agent-beta", "server-1");
        assert_eq!(addr.to_str(), "agent-beta@server-1");

        let parsed = AgentAddr::from_str("agent-beta@server-1");
        assert_eq!(parsed.agent_id, "agent-beta");
        assert_eq!(parsed.machine_id, Some("server-1".to_string()));
    }

    #[test]
    fn test_reply_to_path_stack() {
        let mut rt = ReplyTo::new();
        rt.add_hop("A", "m1");
        rt.add_hop("B", "m2");
        assert_eq!(rt.path, vec!["A@m1", "B@m2"]);

        assert_eq!(rt.pop_next(), Some("B@m2".to_string()));
        assert_eq!(rt.pop_next(), Some("A@m1".to_string()));
        assert_eq!(rt.pop_next(), None);
    }

    #[test]
    fn test_hops_increment() {
        let mut hops = Hops::new(3);
        assert!(hops.increment("A", "m1"));
        assert!(hops.increment("B", "m2"));
        assert!(hops.increment("C", "m3")); // 3rd succeeds with max=3
        assert!(!hops.increment("D", "m4")); // 4th fails
        assert_eq!(hops.count, 4);
    }

    #[test]
    fn test_forward_envelope() {
        let env = build_envelope(
            new_msg_id(),
            new_corr_id(),
            Origin {
                agent_id: Some("orig".to_string()),
                machine_id: Some("m".to_string()),
                human_id: None,
            },
            "A",
            "m1",
            "B",
            "m2",
            Intent::Delegate,
            Some(vec!["X@mX".to_string()]),
            None,
            10,
            "application/json",
            Priority::Normal,
            None,
        );

        let forwarded = forward_envelope(&env, "B", "m2", "C", "m3").unwrap();
        assert_eq!(forwarded.sender.agent_id, "B");
        assert_eq!(forwarded.recipient.agent_id, "C");
        assert!(forwarded.reply_to.as_ref().unwrap().path.contains(&"B@m2".to_string()));
    }

    #[test]
    fn test_reply_envelope() {
        // Path: ["A@m1", "B@m2"], B is the last element (who just forwarded to C)
        // When C replies, pop() gives "B@m2" as recipient
        let env = build_envelope(
            new_msg_id(),
            new_corr_id(),
            Origin::default(),
            "A",
            "m1",
            "C",  // recipient is C (who just forwarded to B)
            "m3",
            Intent::Delegate,
            Some(vec!["A@m1".to_string(), "B@m2".to_string()]),
            None,
            10,
            "application/json",
            Priority::Normal,
            None,
        );

        // B replies - pops B@m2 from path, sends to B
        let reply = reply_envelope(&env, "B", "m2").unwrap();
        assert_eq!(reply.sender.agent_id, "B");
        assert_eq!(reply.recipient.agent_id, "B");  // pops last = B@m2
        assert_eq!(reply.intent, Intent::Reply);
    }

    #[test]
    fn test_ws_frame_roundtrip() {
        let frame = build_ws_frame("str_abc", "msg_123", "msg_123", 0, Some(5), false, serde_json::json!({"x": 1}));
        let parsed = parse_ws_frame(&frame).unwrap();
        assert_eq!(parsed.stream_id, "str_abc");
        assert_eq!(parsed.seq, 0);
        assert_eq!(parsed.total, Some(5));
        assert!(!parsed.final_);
    }
}
