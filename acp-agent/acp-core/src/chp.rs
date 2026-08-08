//! ACP-CHP — Agent Context Handoff Protocol
//!
//! A lightweight, standalone protocol for rich task context transfer between agents.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Extended intents for context handoff workflow
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffIntent {
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
    Error,
}

impl HandoffIntent {
    pub fn as_str(&self) -> &'static str {
        match self {
            HandoffIntent::Handshake => "handshake",
            HandoffIntent::HandshakeAck => "handshake_ack",
            HandoffIntent::HandshakeDecline => "handshake_decline",
            HandoffIntent::HandshakeQuery => "handshake_query",
            HandoffIntent::Handoff => "handoff",
            HandoffIntent::HandoverRequest => "handover_request",
            HandoffIntent::HandoverAccept => "handover_accept",
            HandoffIntent::HandoverDecline => "handover_decline",
            HandoffIntent::HandoverQuery => "handover_query",
            HandoffIntent::Progress => "progress",
            HandoffIntent::Blocked => "blocked",
            HandoffIntent::Complete => "complete",
            HandoffIntent::Error => "error",
        }
    }
}

impl Default for HandoffIntent {
    fn default() -> Self {
        HandoffIntent::Handoff
    }
}

/// Status of the active work item
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Blocked,
    Complete,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::InProgress => "in_progress",
            TaskStatus::Blocked => "blocked",
            TaskStatus::Complete => "complete",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

// ---------------------------------------------------------------------------
// Context Bundle structures
// ---------------------------------------------------------------------------

/// What outcome is needed
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Objective {
    pub outcome: String,
    pub stop_condition: String,
}

impl Objective {
    pub fn new(outcome: impl Into<String>, stop_condition: impl Into<String>) -> Self {
        Self {
            outcome: outcome.into(),
            stop_condition: stop_condition.into(),
        }
    }
}

/// The current task being worked on
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActiveWork {
    pub task_id: String,
    pub description: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default = "default_status")]
    pub current_status: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default = "default_priority")]
    pub priority: String,
}

fn default_status() -> String {
    "pending".to_string()
}

fn default_priority() -> String {
    "normal".to_string()
}

impl ActiveWork {
    pub fn new(task_id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            description: description.into(),
            acceptance_criteria: Vec::new(),
            current_status: default_status(),
            owner: String::new(),
            priority: default_priority(),
        }
    }
}

/// Who owns what decisions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Authority {
    #[serde(default)]
    pub canonical_sources: Vec<String>,
    #[serde(default)]
    pub approvals: Vec<serde_json::Value>,
    #[serde(default)]
    pub notes: String,
}

/// What must/must NOT be done
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Constraints {
    #[serde(default)]
    pub must_do: Vec<String>,
    #[serde(default)]
    pub must_not: Vec<String>,
    #[serde(default)]
    pub architectural: Vec<String>,
    #[serde(default)]
    pub policy: Vec<String>,
}

/// What has been observed/done so far
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Evidence {
    #[serde(default)]
    pub commands: Vec<serde_json::Value>,
    #[serde(default)]
    pub observations: String,
    #[serde(default)]
    pub logs: String,
    #[serde(default)]
    pub environment: String,
    #[serde(default)]
    pub artifacts: Vec<String>,
}

/// What's changed since work started
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChangeState {
    #[serde(default)]
    pub changes: Vec<serde_json::Value>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub unresolved: Vec<String>,
    #[serde(default)]
    pub rollback: String,
}

/// The complete context for a handoff
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContextBundle {
    #[serde(default = "default_version")]
    pub version: String,
    pub objective: Objective,
    pub active_work: ActiveWork,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<Authority>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Constraints>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_state: Option<ChangeState>,
    #[serde(default)]
    pub extras: serde_json::Value,
}

fn default_version() -> String {
    "1.0".to_string()
}

impl ContextBundle {
    pub fn new(objective: Objective, active_work: ActiveWork) -> Self {
        Self {
            version: default_version(),
            objective,
            active_work,
            authority: None,
            constraints: None,
            evidence: None,
            change_state: None,
            extras: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Rough token estimate (1 token ≈ 4 chars)
    pub fn estimate_tokens(&self) -> usize {
        let json = serde_json::to_string(self).unwrap_or_default();
        json.len() / 4
    }
}

// ---------------------------------------------------------------------------
// Handoff Message — wraps ContextBundle for ACP transport
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HandoffMessage {
    #[serde(default = "default_chp_version")]
    pub chp_version: String,
    pub intent: HandoffIntent,
    pub bundle: ContextBundle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default = "default_requires_ack")]
    pub requires_acknowledgment: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_msg_id: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

fn default_chp_version() -> String {
    "1.0".to_string()
}

fn default_requires_ack() -> bool {
    true
}

impl HandoffMessage {
    pub fn new(bundle: ContextBundle, intent: HandoffIntent) -> Self {
        Self {
            chp_version: default_chp_version(),
            intent,
            bundle,
            expires_at: None,
            requires_acknowledgment: default_requires_ack(),
            related_msg_id: None,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    pub fn with_expires_at(mut self, expires_at: impl Into<String>) -> Self {
        self.expires_at = Some(expires_at.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Progress Report — for streaming updates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProgressReport {
    pub msg_id: String,
    #[serde(default = "default_task_status")]
    pub status: TaskStatus,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub seq: u32,
    #[serde(default)]
    pub checkpoint: String,
    #[serde(default)]
    pub percent_complete: u32,
}

fn default_task_status() -> TaskStatus {
    TaskStatus::InProgress
}

impl ProgressReport {
    pub fn new(msg_id: impl Into<String>, status: TaskStatus, message: impl Into<String>) -> Self {
        Self {
            msg_id: msg_id.into(),
            status,
            message: message.into(),
            seq: 0,
            checkpoint: String::new(),
            percent_complete: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Peer Info — for capability exchange during handshake
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PeerInfo {
    pub agent_id: String,
    pub machine_id: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default = "default_online_status")]
    pub status: String,
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_online_status() -> String {
    "online".to_string()
}

fn default_peer_info_version() -> String {
    "1.0".to_string()
}

impl PeerInfo {
    pub fn new(agent_id: impl Into<String>, machine_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            machine_id: machine_id.into(),
            capabilities: Vec::new(),
            status: default_online_status(),
            version: default_peer_info_version(),
        }
    }
}

// ---------------------------------------------------------------------------
// Builder helpers
// ---------------------------------------------------------------------------

/// Build a minimal ContextBundle for handoff
pub fn build_handoff(
    objective: impl Into<String>,
    stop_condition: impl Into<String>,
    task_id: impl Into<String>,
    description: impl Into<String>,
    owner: impl Into<String>,
) -> ContextBundle {
    ContextBundle::new(
        Objective::new(objective, stop_condition),
        ActiveWork::new(task_id, description),
    )
}

/// Build a progress report
pub fn build_progress(
    msg_id: impl Into<String>,
    status: TaskStatus,
    message: impl Into<String>,
) -> ProgressReport {
    ProgressReport::new(msg_id, status, message)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_handoff() {
        let bundle = build_handoff(
            "Fix login timeout",
            "User can login within 3s",
            "FE-042",
            "Debug token refresh race condition",
            "agent-alpha",
        );
        assert_eq!(bundle.objective.outcome, "Fix login timeout");
        assert_eq!(bundle.active_work.task_id, "FE-042");
    }

    #[test]
    fn test_context_bundle_tokens() {
        let bundle = build_handoff("test", "done", "T1", "desc", "owner");
        assert!(bundle.estimate_tokens() > 0);
    }

    #[test]
    fn test_handoff_message_serde() {
        let bundle = build_handoff("test", "done", "T1", "desc", "owner");
        let msg = HandoffMessage::new(bundle, HandoffIntent::Handoff);
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HandoffMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.intent, HandoffIntent::Handoff);
    }
}
