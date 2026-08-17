//! ACP Protocol — message envelope, ULID generation, path building, streaming frames

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use chrono::Utc;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ID Generation
// ---------------------------------------------------------------------------

/// Generate a new message ID with `msg_` prefix.
#[must_use]
pub fn new_msg_id() -> String {
    format!("msg_{}", ulid::Ulid::new().to_string().to_lowercase())
}

/// Generate a new correlation ID.
///
/// Correlation IDs share the message-ID format: the first message of a chain
/// uses its own `msg_id` as the correlation ID for every reply that follows.
#[must_use]
pub fn new_corr_id() -> String {
    new_msg_id()
}

/// Generate a new session ID for a user or automation conversation.
#[must_use]
pub fn new_session_id() -> String {
    format!("ses_{}", ulid::Ulid::new().to_string().to_lowercase())
}

/// Generate a new run ID for one execution within a session.
#[must_use]
pub fn new_run_id() -> String {
    format!("run_{}", ulid::Ulid::new().to_string().to_lowercase())
}

/// Current ACP wire-protocol version.
pub const PROTOCOL_VERSION: &str = "1.1";

/// Wire versions accepted during initialization, newest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["1.1", "1.0"];

/// Select the newest version supported by both peers.
#[must_use]
pub fn negotiate_protocol_version(requested: &[String]) -> Option<&'static str> {
    if requested.is_empty() {
        return Some(PROTOCOL_VERSION);
    }

    SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .find(|version| requested.iter().any(|candidate| candidate == **version))
        .copied()
}

/// Generate a new acknowledgement ID with `ack_` prefix.
#[must_use]
pub fn new_ack_id() -> String {
    format!("ack_{}", ulid::Ulid::new().to_string().to_lowercase())
}

/// Generate a new stream ID with `str_` prefix.
#[must_use]
pub fn new_stream_id() -> String {
    format!("str_{}", ulid::Ulid::new().to_string().to_lowercase())
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// ACP message intent — what action the message represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    /// Hand a unit of work to another agent.
    #[default]
    Delegate,
    /// Answer travelling back along `reply_to.path`.
    Reply,
    /// Confirmation that a hop received or processed a message.
    Ack,
    /// Failure travelling back along `reply_to.path`.
    Error,
    /// Opening frame of a streamed reply.
    StreamStart,
    /// Body frame of a streamed reply.
    StreamChunk,
    /// Closing frame of a streamed reply.
    StreamEnd,
    /// ACP-CHP: offer to exchange capabilities.
    Handshake,
    /// ACP-CHP: capability exchange accepted.
    HandshakeAck,
    /// ACP-CHP: capability exchange refused.
    HandshakeDecline,
    /// ACP-CHP: ask a peer for its capabilities.
    HandshakeQuery,
    /// ACP-CHP: transfer task context to another agent.
    Handoff,
    /// ACP-CHP: ask another agent to take ownership of a task.
    HandoverRequest,
    /// ACP-CHP: ownership request accepted.
    HandoverAccept,
    /// ACP-CHP: ownership request refused.
    HandoverDecline,
    /// ACP-CHP: ask who currently owns a task.
    HandoverQuery,
    /// ACP-CHP: incremental progress on a delegated task.
    Progress,
    /// ACP-CHP: work cannot continue without input.
    Blocked,
    /// ACP-CHP: work finished.
    Complete,
}

impl Intent {
    /// Wire representation of this intent.
    #[must_use]
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

impl fmt::Display for Intent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Message priority — a hint to the recipient's scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// Process when nothing else is queued.
    Low,
    /// Default priority.
    #[default]
    Normal,
    /// Process ahead of queued work.
    High,
}

impl Priority {
    /// Wire representation of this priority.
    #[must_use]
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

/// Who initiated the message chain, carried unchanged across every hop.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Origin {
    /// Agent that started the chain, if it was an agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Machine the originating agent runs on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    /// Human that started the chain, if it was a person.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub human_id: Option<String>,
}

impl Origin {
    /// An origin with no fields set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Human-readable label, preferring the human ID over the agent address.
    #[must_use]
    pub fn label(&self) -> String {
        match (&self.human_id, &self.agent_id, &self.machine_id) {
            (Some(human_id), _, _) => human_id.clone(),
            (None, Some(agent_id), Some(machine_id)) => format!("{agent_id}@{machine_id}"),
            (None, Some(agent_id), None) => agent_id.clone(),
            (None, None, _) => "unknown".to_string(),
        }
    }
}

/// Agent address in `agent_id@machine_id` form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAddr {
    /// Logical agent name, unique within a deployment.
    pub agent_id: String,
    /// Host the agent runs on. Absent when the address is agent-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
}

impl AgentAddr {
    /// Build an address from its two halves.
    pub fn new(agent_id: impl Into<String>, machine_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            machine_id: Some(machine_id.into()),
        }
    }

    /// Render as `agent_id@machine_id`, or just `agent_id` when no machine is set.
    #[must_use]
    pub fn to_str(&self) -> String {
        match &self.machine_id {
            Some(m) => format!("{}@{}", self.agent_id, m),
            None => self.agent_id.clone(),
        }
    }
}

impl FromStr for AgentAddr {
    type Err = std::convert::Infallible;

    /// Parse `agent_id@machine_id`. Input without `@` becomes an agent-only address.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.rsplit_once('@') {
            Some((agent_id, machine_id)) => Self {
                agent_id: agent_id.to_string(),
                machine_id: Some(machine_id.to_string()),
            },
            None => Self {
                agent_id: s.to_string(),
                machine_id: None,
            },
        })
    }
}

impl fmt::Display for AgentAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_str())
    }
}

/// Reply routing path and WebSocket endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReplyTo {
    /// Addresses to unwind on the way back, oldest hop first.
    #[serde(default)]
    pub path: Vec<String>,
    /// Where the originator accepts streamed replies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_endpoint: Option<String>,
}

impl ReplyTo {
    /// An empty reply path with no stream endpoint.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append the current sender to the path when forwarding.
    pub fn add_hop(&mut self, agent_id: &str, machine_id: &str) {
        self.path.push(format!("{agent_id}@{machine_id}"));
    }

    /// Remove and return the next recipient for a reply.
    ///
    /// Pops from the BACK — the last sender is whoever just forwarded here.
    pub fn pop_next(&mut self) -> Option<String> {
        self.path.pop()
    }
}

/// Hop tracking — count, TTL, and audit trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Hops {
    /// Hops taken so far.
    pub count: u32,
    /// Hop ceiling; exceeding it fails the forward.
    pub max: u32,
    /// One entry per hop, in order.
    #[serde(default)]
    pub trace: Vec<HopTraceEntry>,
}

/// One entry in [`Hops::trace`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HopTraceEntry {
    /// Agent that handled this hop.
    pub agent_id: String,
    /// Machine that agent ran on.
    pub machine_id: String,
    /// RFC 3339 timestamp of the hop.
    pub timestamp: String,
}

/// Default hop ceiling applied when a message arrives without one.
pub const DEFAULT_MAX_HOPS: u32 = 10;

impl Hops {
    /// Fresh hop tracking with the given ceiling.
    #[must_use]
    pub fn new(max: u32) -> Self {
        Self {
            count: 0,
            max,
            trace: Vec::new(),
        }
    }

    /// Increment the hop count and record a trace entry.
    ///
    /// Returns `false` once the ceiling has been exceeded.
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
        Self::new(DEFAULT_MAX_HOPS)
    }
}

/// The envelope — the core ACP message metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Envelope {
    /// Unique ID of this message, stable across forwards.
    pub msg_id: String,
    /// Groups every message belonging to one exchange.
    #[serde(default)]
    pub corr_id: Option<String>,
    /// User or automation conversation this message belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// One execution within `session_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Who started the chain.
    #[serde(default)]
    pub origin: Option<Origin>,
    /// Who sent this hop.
    pub sender: AgentAddr,
    /// Who this hop is addressed to.
    pub recipient: AgentAddr,
    /// Route a reply must unwind.
    #[serde(default)]
    pub reply_to: Option<ReplyTo>,
    /// What action this message represents.
    #[serde(default)]
    pub intent: Intent,
    /// MIME type of `payload`.
    #[serde(default)]
    pub content_type: Option<String>,
    /// Scheduling hint for the recipient.
    #[serde(default)]
    pub priority: Priority,
    /// RFC 3339 instant after which the message is stale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    /// Failure description, set on `Error` intent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Hop count and audit trail.
    #[serde(default)]
    pub hops: Option<Hops>,
    /// Any envelope fields this version does not model, preserved on forward.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// A full ACP message — envelope plus payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Routing and protocol metadata.
    pub envelope: Envelope,
    /// Application data, interpreted per `envelope.content_type`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

impl Message {
    /// A message carrying metadata only.
    #[must_use]
    pub fn new(envelope: Envelope) -> Self {
        Self {
            envelope,
            payload: None,
        }
    }

    /// A message carrying `payload`.
    #[must_use]
    pub fn with_payload(envelope: Envelope, payload: serde_json::Value) -> Self {
        Self {
            envelope,
            payload: Some(payload),
        }
    }

    /// Serialize to the JSON wire form.
    ///
    /// # Errors
    /// Fails if `payload` contains a map with non-string keys.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    /// Parse from the JSON wire form.
    ///
    /// # Errors
    /// Fails if `raw` is not valid JSON or is missing a required envelope field.
    pub fn from_json(raw: &str) -> serde_json::Result<Self> {
        serde_json::from_str(raw)
    }
}

// ---------------------------------------------------------------------------
// Stream frames
// ---------------------------------------------------------------------------

/// WebSocket streaming frame header.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StreamFrame {
    /// Groups every frame of one stream.
    pub stream_id: String,
    /// Message this stream is answering.
    pub msg_id: String,
    /// Correlation ID of the exchange.
    #[serde(default)]
    pub corr_id: Option<String>,
    /// Session this stream belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Run this stream belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Zero-based frame index.
    pub seq: u32,
    /// Total frame count, when known up front.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
    /// True on the last frame.
    pub final_: bool,
    /// RFC 3339 send time.
    #[serde(default = "iso_now")]
    pub timestamp: String,
}

/// Everything needed to emit one frame of a stream.
#[derive(Debug, Clone)]
pub struct StreamChunk<'a> {
    /// Groups every frame of one stream.
    pub stream_id: &'a str,
    /// Message this stream is answering.
    pub msg_id: &'a str,
    /// Correlation ID of the exchange.
    pub corr_id: &'a str,
    /// Session this stream belongs to.
    pub session_id: Option<&'a str>,
    /// Run this stream belongs to.
    pub run_id: Option<&'a str>,
    /// Zero-based frame index.
    pub seq: u32,
    /// Total frame count, when known up front.
    pub total: Option<u32>,
    /// True on the last frame.
    pub final_: bool,
    /// Frame body.
    pub data: serde_json::Value,
}

fn iso_now() -> String {
    Utc::now().to_rfc3339()
}

/// Build a WebSocket frame for streaming.
#[must_use]
pub fn build_ws_frame(chunk: StreamChunk<'_>) -> serde_json::Value {
    let StreamChunk {
        stream_id,
        msg_id,
        corr_id,
        seq,
        total,
        final_,
        data,
        session_id,
        run_id,
    } = chunk;

    serde_json::json!({
        "frame": StreamFrame {
            stream_id: stream_id.to_string(),
            msg_id: msg_id.to_string(),
            corr_id: Some(corr_id.to_string()),
            session_id: session_id.map(String::from),
            run_id: run_id.map(String::from),
            seq,
            total,
            final_,
            timestamp: iso_now(),
        },
        "data": data
    })
}

/// Parse a WebSocket frame header from a JSON value.
///
/// Returns `None` when `raw` has no `frame` object or is missing a required field.
#[must_use]
pub fn parse_ws_frame(raw: &serde_json::Value) -> Option<StreamFrame> {
    let frame = raw.get("frame")?;
    Some(StreamFrame {
        stream_id: frame.get("stream_id")?.as_str()?.to_string(),
        msg_id: frame.get("msg_id")?.as_str()?.to_string(),
        corr_id: frame
            .get("corr_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        session_id: frame
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        run_id: frame
            .get("run_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        seq: u32::try_from(frame.get("seq")?.as_u64()?).ok()?,
        total: frame
            .get("total")
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok()),
        final_: frame
            .get("final_")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        timestamp: frame
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map_or_else(iso_now, String::from),
    })
}

// ---------------------------------------------------------------------------
// Envelope builders
// ---------------------------------------------------------------------------

/// Everything that varies between freshly-built envelopes.
///
/// Grouping these keeps [`build_envelope`] readable at the call site — fourteen
/// positional arguments were indistinguishable from one another.
#[derive(Debug, Clone)]
pub struct NewEnvelope<'a> {
    /// Unique ID for the message.
    pub msg_id: String,
    /// Correlation ID for the exchange.
    pub corr_id: String,
    /// User or automation conversation this message belongs to.
    pub session_id: Option<String>,
    /// One execution within `session_id`.
    pub run_id: Option<String>,
    /// Who started the chain.
    pub origin: Origin,
    /// Sending agent.
    pub sender: (&'a str, &'a str),
    /// Receiving agent.
    pub recipient: (&'a str, &'a str),
    /// What action the message represents.
    pub intent: Intent,
    /// Seed reply path; `None` leaves `reply_to` unset.
    pub reply_to_path: Option<Vec<String>>,
    /// Where the sender accepts streamed replies.
    pub reply_to_ws_endpoint: Option<String>,
    /// Hop ceiling for the chain.
    pub hops_max: u32,
    /// MIME type of the payload.
    pub content_type: &'a str,
    /// Scheduling hint.
    pub priority: Priority,
    /// RFC 3339 instant after which the message is stale.
    pub deadline: Option<String>,
}

/// Build a new envelope for a fresh message (not a forward).
#[must_use]
pub fn build_envelope(spec: NewEnvelope<'_>) -> Envelope {
    let reply_to = spec.reply_to_path.map(|path| ReplyTo {
        path,
        ws_endpoint: spec.reply_to_ws_endpoint,
    });

    Envelope {
        msg_id: spec.msg_id,
        corr_id: Some(spec.corr_id),
        session_id: spec.session_id,
        run_id: spec.run_id,
        origin: Some(spec.origin),
        sender: AgentAddr::new(spec.sender.0, spec.sender.1),
        recipient: AgentAddr::new(spec.recipient.0, spec.recipient.1),
        reply_to,
        intent: spec.intent,
        content_type: Some(spec.content_type.to_string()),
        priority: spec.priority,
        deadline: spec.deadline,
        error: None,
        hops: Some(Hops::new(spec.hops_max)),
        extra: HashMap::new(),
    }
}

/// Clone an envelope for forwarding to the next hop (delegation direction).
///
/// Increments the hop count, appends the current sender to `reply_to.path`, and
/// rewrites sender and recipient.
///
/// # Errors
/// Returns [`HopsExceededError`] once the chain has used up its hop ceiling.
pub fn forward_envelope(
    envelope: &Envelope,
    new_sender_agent_id: &str,
    new_sender_machine_id: &str,
    new_recipient_agent_id: &str,
    new_recipient_machine_id: &str,
) -> Result<Envelope, HopsExceededError> {
    let mut hops = envelope.hops.clone().unwrap_or_default();
    if !hops.increment(new_sender_agent_id, new_sender_machine_id) {
        return Err(HopsExceededError {
            max: hops.max,
            at: format!("{new_sender_agent_id}@{new_sender_machine_id}"),
        });
    }

    let mut new_reply_to = envelope.reply_to.clone().unwrap_or_default();
    new_reply_to.add_hop(new_sender_agent_id, new_sender_machine_id);

    Ok(Envelope {
        msg_id: envelope.msg_id.clone(),
        corr_id: envelope.corr_id.clone(),
        session_id: envelope.session_id.clone(),
        run_id: envelope.run_id.clone(),
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
    })
}

/// Clone an envelope for sending a reply back along `reply_to.path`.
///
/// Pops the LAST entry from the path — the agent that just forwarded here — and
/// switches the intent to [`Intent::Reply`].
///
/// # Errors
/// Returns [`ReplyPathEmptyError`] when the path is exhausted, or when the hop
/// ceiling is reached (the hop failure is folded into the same error type).
pub fn reply_envelope(
    envelope: &Envelope,
    new_sender_agent_id: &str,
    new_sender_machine_id: &str,
) -> Result<Envelope, ReplyPathEmptyError> {
    let mut reply_to = envelope.reply_to.clone().unwrap_or_default();
    let next_hop = reply_to
        .pop_next()
        .ok_or_else(|| ReplyPathEmptyError(envelope.msg_id.clone()))?;

    let (next_agent_id, next_machine_id) = next_hop.rsplit_once('@').unwrap_or((&next_hop, ""));

    let mut hops = envelope.hops.clone().unwrap_or_default();
    if !hops.increment(new_sender_agent_id, new_sender_machine_id) {
        return Err(HopsExceededError {
            max: hops.max,
            at: format!("{new_sender_agent_id}@{new_sender_machine_id}"),
        }
        .into());
    }

    Ok(Envelope {
        msg_id: envelope.msg_id.clone(),
        corr_id: envelope.corr_id.clone(),
        session_id: envelope.session_id.clone(),
        run_id: envelope.run_id.clone(),
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

/// A forward or reply would have pushed the chain past its hop ceiling.
#[derive(Debug, thiserror::Error)]
#[error("Max hops ({max}) exceeded at {at}")]
pub struct HopsExceededError {
    /// Ceiling the chain was configured with.
    pub max: u32,
    /// Address of the hop that would have exceeded it.
    pub at: String,
}

/// A reply had nowhere to go because `reply_to.path` was empty.
#[derive(Debug, thiserror::Error)]
#[error("No reply_to.path to route reply for {0}")]
pub struct ReplyPathEmptyError(pub String);

impl From<HopsExceededError> for ReplyPathEmptyError {
    fn from(error: HopsExceededError) -> Self {
        ReplyPathEmptyError(error.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope_between(
        sender: (&str, &str),
        recipient: (&str, &str),
        reply_to_path: Vec<String>,
    ) -> Envelope {
        build_envelope(NewEnvelope {
            msg_id: new_msg_id(),
            corr_id: new_corr_id(),
            session_id: None,
            run_id: None,
            origin: Origin::default(),
            sender,
            recipient,
            intent: Intent::Delegate,
            reply_to_path: Some(reply_to_path),
            reply_to_ws_endpoint: None,
            hops_max: 10,
            content_type: "application/json",
            priority: Priority::Normal,
            deadline: None,
        })
    }

    #[test]
    fn new_msg_id_is_prefixed_ulid() {
        let id = new_msg_id();
        assert!(id.starts_with("msg_"));
        assert_eq!(id.len(), 4 + 26); // "msg_" + 26 char ULID
    }

    #[test]
    fn agent_addr_renders_agent_at_machine() {
        assert_eq!(
            AgentAddr::new("agent-beta", "server-1").to_str(),
            "agent-beta@server-1"
        );
    }

    #[test]
    fn agent_addr_parses_agent_at_machine() {
        let parsed: AgentAddr = "agent-beta@server-1".parse().unwrap();
        assert_eq!(parsed.agent_id, "agent-beta");
        assert_eq!(parsed.machine_id, Some("server-1".to_string()));
    }

    #[test]
    fn agent_addr_without_machine_parses_as_agent_only() {
        let parsed: AgentAddr = "agent-beta".parse().unwrap();
        assert_eq!(parsed.machine_id, None);
    }

    #[test]
    fn reply_path_pops_the_most_recent_hop_first() {
        let mut rt = ReplyTo::new();
        rt.add_hop("A", "m1");
        rt.add_hop("B", "m2");
        assert_eq!(rt.path, vec!["A@m1", "B@m2"]);

        assert_eq!(rt.pop_next(), Some("B@m2".to_string()));
        assert_eq!(rt.pop_next(), Some("A@m1".to_string()));
        assert_eq!(rt.pop_next(), None);
    }

    #[test]
    fn hop_increment_fails_once_past_the_ceiling() {
        let mut hops = Hops::new(3);
        assert!(hops.increment("A", "m1"));
        assert!(hops.increment("B", "m2"));
        assert!(hops.increment("C", "m3")); // 3rd succeeds with max=3
        assert!(!hops.increment("D", "m4")); // 4th fails
        assert_eq!(hops.count, 4);
    }

    #[test]
    fn forward_rewrites_sender_and_recipient() {
        let env = envelope_between(("A", "m1"), ("B", "m2"), vec!["X@mX".to_string()]);

        let forwarded = forward_envelope(&env, "B", "m2", "C", "m3").unwrap();
        assert_eq!(forwarded.sender.agent_id, "B");
        assert_eq!(forwarded.recipient.agent_id, "C");
    }

    #[test]
    fn forwarding_preserves_session_and_run_context() {
        let mut env = envelope_between(("A", "m1"), ("B", "m2"), vec!["A@m1".to_string()]);
        env.session_id = Some("ses_123".to_string());
        env.run_id = Some("run_123".to_string());

        let forwarded = forward_envelope(&env, "B", "m2", "C", "m3").unwrap();

        assert_eq!(forwarded.session_id.as_deref(), Some("ses_123"));
        assert_eq!(forwarded.run_id.as_deref(), Some("run_123"));
    }

    #[test]
    fn forward_appends_the_forwarder_to_the_reply_path() {
        let env = envelope_between(("A", "m1"), ("B", "m2"), vec!["X@mX".to_string()]);

        let forwarded = forward_envelope(&env, "B", "m2", "C", "m3").unwrap();
        let path = &forwarded.reply_to.as_ref().unwrap().path;
        assert!(path.contains(&"B@m2".to_string()));
    }

    #[test]
    fn forward_fails_once_hops_are_exhausted() {
        let mut env = envelope_between(("A", "m1"), ("B", "m2"), Vec::new());
        env.hops = Some(Hops {
            count: 10,
            max: 10,
            trace: Vec::new(),
        });

        let error = forward_envelope(&env, "B", "m2", "C", "m3").unwrap_err();
        assert_eq!(error.to_string(), "Max hops (10) exceeded at B@m2");
    }

    #[test]
    fn reply_routes_to_the_last_path_entry() {
        // Path ["A@m1", "B@m2"]: B forwarded to C, so C's reply pops B@m2.
        let env = envelope_between(
            ("A", "m1"),
            ("C", "m3"),
            vec!["A@m1".to_string(), "B@m2".to_string()],
        );

        let reply = reply_envelope(&env, "C", "m3").unwrap();
        assert_eq!(reply.recipient.agent_id, "B");
    }

    #[test]
    fn reply_switches_intent_to_reply() {
        let env = envelope_between(("A", "m1"), ("C", "m3"), vec!["A@m1".to_string()]);

        let reply = reply_envelope(&env, "C", "m3").unwrap();
        assert_eq!(reply.intent, Intent::Reply);
    }

    #[test]
    fn reply_fails_when_the_path_is_empty() {
        let env = envelope_between(("A", "m1"), ("C", "m3"), Vec::new());

        let error = reply_envelope(&env, "C", "m3").unwrap_err();
        assert!(error.to_string().starts_with("No reply_to.path"));
    }

    #[test]
    fn ws_frame_survives_a_build_parse_roundtrip() {
        let frame = build_ws_frame(StreamChunk {
            stream_id: "str_abc",
            msg_id: "msg_123",
            corr_id: "msg_123",
            session_id: Some("ses_123"),
            run_id: Some("run_123"),
            seq: 0,
            total: Some(5),
            final_: false,
            data: serde_json::json!({"x": 1}),
        });

        let parsed = parse_ws_frame(&frame).unwrap();
        assert_eq!(parsed.stream_id, "str_abc");
        assert_eq!(parsed.total, Some(5));
        assert_eq!(parsed.session_id.as_deref(), Some("ses_123"));
        assert_eq!(parsed.run_id.as_deref(), Some("run_123"));
    }

    #[test]
    fn negotiation_prefers_the_newest_common_version() {
        assert_eq!(
            negotiate_protocol_version(&["1.0".to_string(), "1.1".to_string()]),
            Some("1.1")
        );
        assert_eq!(negotiate_protocol_version(&["0.9".to_string()]), None);
        assert_eq!(negotiate_protocol_version(&[]), Some(PROTOCOL_VERSION));
    }
}
