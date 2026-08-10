//! ACP-CHP — Agent Context Handoff Protocol
//!
//! A lightweight, standalone protocol for rich task context transfer between agents.

use serde::{Deserialize, Serialize};

/// Version this crate speaks, stamped onto bundles and handoff messages.
pub const CHP_VERSION: &str = "1.0";

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Extended intents for the context-handoff workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffIntent {
    /// Offer to exchange capabilities.
    Handshake,
    /// Capability exchange accepted.
    HandshakeAck,
    /// Capability exchange refused.
    HandshakeDecline,
    /// Ask a peer for its capabilities.
    HandshakeQuery,
    /// Transfer task context without transferring ownership.
    #[default]
    Handoff,
    /// Ask another agent to take ownership of a task.
    HandoverRequest,
    /// Ownership request accepted.
    HandoverAccept,
    /// Ownership request refused.
    HandoverDecline,
    /// Ask who currently owns a task.
    HandoverQuery,
    /// Incremental progress on a delegated task.
    Progress,
    /// Work cannot continue without input.
    Blocked,
    /// Work finished.
    Complete,
    /// Work failed.
    Error,
}

impl HandoffIntent {
    /// Wire representation of this intent.
    #[must_use]
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

/// Status of the active work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Not started.
    #[default]
    Pending,
    /// Being worked on.
    InProgress,
    /// Waiting on something outside the agent's control.
    Blocked,
    /// Finished successfully.
    Complete,
    /// Finished unsuccessfully.
    Failed,
    /// Abandoned before completion.
    Cancelled,
}

impl TaskStatus {
    /// Wire representation of this status.
    #[must_use]
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

/// What outcome is needed, and how the receiver knows it is done.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Objective {
    /// The result being asked for.
    pub outcome: String,
    /// The condition under which work should stop.
    pub stop_condition: String,
}

impl Objective {
    /// Build an objective from its outcome and stop condition.
    pub fn new(outcome: impl Into<String>, stop_condition: impl Into<String>) -> Self {
        Self {
            outcome: outcome.into(),
            stop_condition: stop_condition.into(),
        }
    }
}

/// The current task being worked on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActiveWork {
    /// Identifier in the sender's tracker.
    pub task_id: String,
    /// What the task involves.
    pub description: String,
    /// Conditions the result must satisfy.
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    /// One of [`TaskStatus`], as a wire string.
    #[serde(default = "default_status")]
    pub current_status: String,
    /// Agent currently responsible for the task.
    #[serde(default)]
    pub owner: String,
    /// Scheduling hint, matching the ACP priority names.
    #[serde(default = "default_priority")]
    pub priority: String,
}

fn default_status() -> String {
    TaskStatus::Pending.as_str().to_string()
}

fn default_priority() -> String {
    "normal".to_string()
}

impl ActiveWork {
    /// A pending, unowned task with no acceptance criteria.
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

    /// Set the agent responsible for the task.
    #[must_use]
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = owner.into();
        self
    }
}

/// Who owns which decisions on this task.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Authority {
    /// Documents that settle disputes about the task.
    #[serde(default)]
    pub canonical_sources: Vec<String>,
    /// Approvals already granted, in a shape the sender defines.
    #[serde(default)]
    pub approvals: Vec<serde_json::Value>,
    /// Anything else the receiver needs to know about authority.
    #[serde(default)]
    pub notes: String,
}

/// What must and must not be done.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Constraints {
    /// Required steps.
    #[serde(default)]
    pub must_do: Vec<String>,
    /// Forbidden actions.
    #[serde(default)]
    pub must_not: Vec<String>,
    /// Structural rules the solution has to respect.
    #[serde(default)]
    pub architectural: Vec<String>,
    /// Organizational or legal rules.
    #[serde(default)]
    pub policy: Vec<String>,
}

/// What has been observed or done so far.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Evidence {
    /// Commands run, in a shape the sender defines.
    #[serde(default)]
    pub commands: Vec<serde_json::Value>,
    /// What the sender concluded from them.
    #[serde(default)]
    pub observations: String,
    /// Relevant log excerpts.
    #[serde(default)]
    pub logs: String,
    /// Where the work was carried out.
    #[serde(default)]
    pub environment: String,
    /// Files or artifacts produced.
    #[serde(default)]
    pub artifacts: Vec<String>,
}

/// What has changed since work started.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChangeState {
    /// Edits made, in a shape the sender defines.
    #[serde(default)]
    pub changes: Vec<serde_json::Value>,
    /// Decisions taken along the way.
    #[serde(default)]
    pub decisions: Vec<String>,
    /// Questions still open.
    #[serde(default)]
    pub unresolved: Vec<String>,
    /// How to undo the work so far.
    #[serde(default)]
    pub rollback: String,
}

/// The complete context for a handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContextBundle {
    /// CHP version this bundle was built against.
    #[serde(default = "default_version")]
    pub version: String,
    /// What outcome is needed.
    pub objective: Objective,
    /// The task itself.
    pub active_work: ActiveWork,
    /// Who owns which decisions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<Authority>,
    /// What must and must not be done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Constraints>,
    /// What has been observed so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
    /// What has changed so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_state: Option<ChangeState>,
    /// Anything the sender needs to carry that CHP does not model.
    #[serde(default)]
    pub extras: serde_json::Value,
}

fn default_version() -> String {
    CHP_VERSION.to_string()
}

impl ContextBundle {
    /// A bundle carrying only the required sections.
    #[must_use]
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

    /// Rough token estimate for the serialized bundle (1 token ≈ 4 chars).
    ///
    /// Returns `0` for a bundle that cannot be serialized.
    #[must_use]
    pub fn estimate_tokens(&self) -> usize {
        serde_json::to_string(self).unwrap_or_default().len() / 4
    }
}

// ---------------------------------------------------------------------------
// Handoff Message — wraps ContextBundle for ACP transport
// ---------------------------------------------------------------------------

/// A [`ContextBundle`] addressed for transport as an ACP payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HandoffMessage {
    /// CHP version the sender speaks.
    #[serde(default = "default_version")]
    pub chp_version: String,
    /// What the sender is asking for.
    pub intent: HandoffIntent,
    /// The context being transferred.
    pub bundle: ContextBundle,
    /// RFC 3339 instant after which the handoff is stale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Whether the sender expects an explicit acknowledgement.
    #[serde(default = "default_requires_ack")]
    pub requires_acknowledgment: bool,
    /// Message this handoff continues, when it is part of a chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_msg_id: Option<String>,
    /// Anything else the sender needs to carry.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

fn default_requires_ack() -> bool {
    true
}

impl HandoffMessage {
    /// Wrap a bundle with the given intent, requiring acknowledgement.
    #[must_use]
    pub fn new(bundle: ContextBundle, intent: HandoffIntent) -> Self {
        Self {
            chp_version: default_version(),
            intent,
            bundle,
            expires_at: None,
            requires_acknowledgment: default_requires_ack(),
            related_msg_id: None,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Set the instant after which the handoff should be ignored.
    #[must_use]
    pub fn with_expires_at(mut self, expires_at: impl Into<String>) -> Self {
        self.expires_at = Some(expires_at.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Progress Report — for streaming updates
// ---------------------------------------------------------------------------

/// An incremental status update on a delegated task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProgressReport {
    /// Message whose work this reports on.
    pub msg_id: String,
    /// Where the task stands.
    #[serde(default = "default_task_status")]
    pub status: TaskStatus,
    /// Human-readable note about the update.
    #[serde(default)]
    pub message: String,
    /// Zero-based index within a series of reports.
    #[serde(default)]
    pub seq: u32,
    /// Named milestone the task has reached.
    #[serde(default)]
    pub checkpoint: String,
    /// Completion estimate, 0–100.
    #[serde(default)]
    pub percent_complete: u32,
}

fn default_task_status() -> TaskStatus {
    TaskStatus::InProgress
}

impl ProgressReport {
    /// A first report (`seq` 0) with no checkpoint.
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

/// What one agent tells another about itself during a handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PeerInfo {
    /// Logical agent name.
    pub agent_id: String,
    /// Host the agent runs on.
    pub machine_id: String,
    /// What the agent advertises it can do.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Availability, e.g. `"online"`.
    #[serde(default = "default_online_status")]
    pub status: String,
    /// CHP version the agent speaks.
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_online_status() -> String {
    "online".to_string()
}

impl PeerInfo {
    /// An online peer with no advertised capabilities.
    pub fn new(agent_id: impl Into<String>, machine_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            machine_id: machine_id.into(),
            capabilities: Vec::new(),
            status: default_online_status(),
            version: default_version(),
        }
    }
}

// ---------------------------------------------------------------------------
// Builder helpers
// ---------------------------------------------------------------------------

/// Build a minimal [`ContextBundle`] for a handoff.
#[must_use]
pub fn build_handoff(
    objective: impl Into<String>,
    stop_condition: impl Into<String>,
    task_id: impl Into<String>,
    description: impl Into<String>,
    owner: impl Into<String>,
) -> ContextBundle {
    ContextBundle::new(
        Objective::new(objective, stop_condition),
        ActiveWork::new(task_id, description).with_owner(owner),
    )
}

/// Build a first [`ProgressReport`] for a message.
#[must_use]
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
    fn build_handoff_carries_the_objective() {
        let bundle = build_handoff(
            "Fix login timeout",
            "User can login within 3s",
            "FE-042",
            "Debug token refresh race condition",
            "agent-alpha",
        );

        assert_eq!(bundle.objective.outcome, "Fix login timeout");
    }

    #[test]
    fn build_handoff_carries_the_task_id() {
        let bundle = build_handoff("test", "done", "FE-042", "desc", "agent-alpha");

        assert_eq!(bundle.active_work.task_id, "FE-042");
    }

    #[test]
    fn build_handoff_records_the_owner() {
        let bundle = build_handoff("test", "done", "FE-042", "desc", "agent-alpha");

        assert_eq!(bundle.active_work.owner, "agent-alpha");
    }

    #[test]
    fn a_bundle_estimates_a_nonzero_token_count() {
        let bundle = build_handoff("test", "done", "T1", "desc", "owner");

        assert!(bundle.estimate_tokens() > 0);
    }

    #[test]
    fn handoff_message_survives_a_serde_roundtrip() {
        let bundle = build_handoff("test", "done", "T1", "desc", "owner");
        let msg = HandoffMessage::new(bundle, HandoffIntent::Handoff);

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HandoffMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.intent, HandoffIntent::Handoff);
    }
}
