//! ACP Config — peer configuration loaded from `acp-peers.yaml`

use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::security::{PeerAuth, AUTH_TYPE_SIGNED_TOKEN};

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Identity and endpoints of the agent reading this config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ThisAgent {
    /// Logical agent name.
    pub agent_id: String,
    /// Host the agent runs on; inferred from the hostname when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    /// Base URL this agent serves ACP on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_endpoint: Option<String>,
    /// URL this agent accepts streamed replies on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_endpoint: Option<String>,
    /// What this agent advertises it can do.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// A peer this agent can address.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Peer {
    /// Logical agent name.
    pub agent_id: String,
    /// Host the peer runs on.
    pub machine_id: String,
    /// Base URL to send ACP requests to.
    pub http_endpoint: String,
    /// URL to open streams against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_endpoint: Option<String>,
    /// What the peer advertises it can do.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// How to authenticate to this peer; signed-token when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<PeerAuth>,
}

impl Peer {
    /// Render as `agent_id@machine_id`.
    #[must_use]
    pub fn addr(&self) -> String {
        format!("{}@{}", self.agent_id, self.machine_id)
    }
}

/// Transport security defaults applied to every peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SecurityConfig {
    /// Auth scheme used by peers that do not override it.
    #[serde(default = "default_auth_type")]
    pub default_auth_type: String,
    /// Lifetime of tokens this agent mints.
    #[serde(default = "default_token_ttl")]
    pub token_ttl_seconds: u64,
    /// Whether plaintext peer endpoints are refused.
    #[serde(default = "default_require_https")]
    pub require_https: bool,
    /// Lowest acceptable TLS version.
    #[serde(default = "default_min_tls")]
    pub min_tls_version: String,
}

fn default_auth_type() -> String {
    AUTH_TYPE_SIGNED_TOKEN.to_string()
}
fn default_token_ttl() -> u64 {
    3600
}
fn default_require_https() -> bool {
    true
}
fn default_min_tls() -> String {
    "1.3".to_string()
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            default_auth_type: default_auth_type(),
            token_ttl_seconds: default_token_ttl(),
            require_https: default_require_https(),
            min_tls_version: default_min_tls(),
        }
    }
}

/// Exponential-backoff policy for outbound sends.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RetryConfig {
    /// Total send attempts, including the first.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// Delay before the second attempt.
    #[serde(default = "default_initial_backoff")]
    pub initial_backoff_ms: u64,
    /// Ceiling the backoff grows to.
    #[serde(default = "default_max_backoff")]
    pub max_backoff_ms: u64,
    /// Factor applied to the delay after each failure.
    #[serde(default = "default_backoff_multiplier")]
    pub backoff_multiplier: f64,
}

fn default_max_attempts() -> u32 {
    3
}
fn default_initial_backoff() -> u64 {
    1000
}
fn default_max_backoff() -> u64 {
    30_000
}
fn default_backoff_multiplier() -> f64 {
    2.0
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            initial_backoff_ms: default_initial_backoff(),
            max_backoff_ms: default_max_backoff(),
            backoff_multiplier: default_backoff_multiplier(),
        }
    }
}

/// Deadlines applied to the stages of a message's life.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TimeoutsConfig {
    /// How long to wait for the next hop to confirm receipt.
    #[serde(default = "default_hop_ack")]
    pub hop_ack_ms: u64,
    /// How long to wait for the recipient to confirm processing.
    #[serde(default = "default_process_ack")]
    pub process_ack_ms: u64,
    /// How long to wait for a stream to open.
    #[serde(default = "default_stream_init")]
    pub stream_init_ms: u64,
    /// How long an idle stream is held open.
    #[serde(default = "default_idle_close")]
    pub idle_close_ms: u64,
}

fn default_hop_ack() -> u64 {
    5_000
}
fn default_process_ack() -> u64 {
    300_000
}
fn default_stream_init() -> u64 {
    10_000
}
fn default_idle_close() -> u64 {
    60_000
}

impl Default for TimeoutsConfig {
    fn default() -> Self {
        Self {
            hop_ack_ms: default_hop_ack(),
            process_ack_ms: default_process_ack(),
            stream_init_ms: default_stream_init(),
            idle_close_ms: default_idle_close(),
        }
    }
}

/// The whole of `acp-peers.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ACPConfig {
    /// Schema version of this file.
    #[serde(default)]
    pub config_version: u32,
    /// When the file was last written, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Identity of the agent reading the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub this_agent: Option<ThisAgent>,
    /// Every peer this agent can address.
    #[serde(default)]
    pub peers: Vec<Peer>,
    /// Transport security defaults.
    #[serde(default)]
    pub security: SecurityConfig,
    /// Outbound retry policy.
    #[serde(default)]
    pub retry: RetryConfig,
    /// Stage deadlines.
    #[serde(default)]
    pub timeouts: TimeoutsConfig,
}

impl ACPConfig {
    /// Find a peer by agent ID, ignoring which machine it runs on.
    #[must_use]
    pub fn get_peer(&self, agent_id: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.agent_id == agent_id)
    }

    /// Find a peer by its full `agent_id@machine_id` address.
    #[must_use]
    pub fn get_peer_by_addr(&self, addr: &str) -> Option<&Peer> {
        let (agent_id, machine_id) = addr.rsplit_once('@').unwrap_or((addr, ""));
        self.peers
            .iter()
            .find(|p| p.agent_id == agent_id && p.machine_id == machine_id)
    }
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

/// Environment variable naming an explicit config path.
pub const CONFIG_ENV: &str = "ACP_PEERS_PATH";

/// File name searched for in the default locations.
pub const DEFAULT_CONFIG_NAME: &str = "acp-peers.yaml";

/// Load ACP peer config from a YAML file.
///
/// Resolution order, first hit wins:
/// 1. `path`, when given
/// 2. `ACP_PEERS_PATH`
/// 3. `./acp-peers.yaml`
/// 4. `~/.acp/acp-peers.yaml`
/// 5. `/etc/acp/acp-peers.yaml`
///
/// # Errors
/// - [`ConfigError::NotFound`] when no candidate path exists.
/// - [`ConfigError::Io`] / [`ConfigError::Parse`] from [`load_from_path`].
pub fn load_config(path: Option<&str>) -> Result<ACPConfig, ConfigError> {
    let candidates: Vec<PathBuf> = match (path, env::var(CONFIG_ENV)) {
        (Some(p), _) => vec![PathBuf::from(p)],
        (None, Ok(p)) => vec![PathBuf::from(p)],
        (None, Err(_)) => default_config_paths(),
    };

    let Some(found) = candidates.iter().find(|p| p.exists()) else {
        return Err(ConfigError::NotFound(
            candidates
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
        ));
    };

    load_from_path(found)
}

fn default_config_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(".").join(DEFAULT_CONFIG_NAME)];
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".acp").join(DEFAULT_CONFIG_NAME));
    }
    paths.push(PathBuf::from("/etc/acp").join(DEFAULT_CONFIG_NAME));
    paths
}

/// Load config from a specific path.
///
/// # Errors
/// - [`ConfigError::Io`] when the file cannot be read.
/// - [`ConfigError::Parse`] when it is not valid ACP YAML.
pub fn load_from_path(path: &Path) -> Result<ACPConfig, ConfigError> {
    let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Io(e.to_string()))?;
    serde_yaml::from_str(&content).map_err(|e| ConfigError::Parse(e.to_string()))
}

/// Resolve `this_agent`, inferring `machine_id` from the hostname when unset.
///
/// # Errors
/// Returns [`ConfigError::MissingThisAgent`] when the section is absent or has
/// no `http_endpoint` — an agent with no address cannot be replied to.
pub fn resolve_this_agent(config: &ACPConfig) -> Result<ThisAgent, ConfigError> {
    let this_agent = config
        .this_agent
        .as_ref()
        .ok_or(ConfigError::MissingThisAgent)?;

    let mut resolved = this_agent.clone();
    if resolved.machine_id.is_none() {
        resolved.machine_id = Some(local_machine_id());
    }
    if resolved.http_endpoint.is_none() {
        return Err(ConfigError::MissingThisAgent);
    }
    Ok(resolved)
}

/// This host's name, or `"unknown"` when it cannot be read.
fn local_machine_id() -> String {
    hostname::get().map_or_else(
        |_| "unknown".to_string(),
        |name| name.to_string_lossy().to_string(),
    )
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a config could not be loaded.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// None of the candidate paths existed.
    #[error("No ACP config found. Checked: {0:?}")]
    NotFound(Vec<String>),

    /// The file exists but could not be read.
    #[error("IO error: {0}")]
    Io(String),

    /// The file is not valid ACP YAML.
    #[error("Parse error: {0}")]
    Parse(String),

    /// `this_agent` is missing, or has no `http_endpoint`.
    #[error("this_agent not set in config")]
    MissingThisAgent,
}

// ---------------------------------------------------------------------------
// Config template generator
// ---------------------------------------------------------------------------

/// Skeleton `acp-peers.yaml`, with `{agent_id}` / `{machine_id}` / `{updated_at}`
/// placeholders filled in by [`generate_config`].
pub const ACP_CONFIG_TEMPLATE: &str = r#"# ACP Peer Configuration
# Generated for {agent_id}@{machine_id}

config_version: 1
updated_at: "{updated_at}"

this_agent:
  agent_id: "{agent_id}"
  machine_id: "{machine_id}"
  http_endpoint: "https://{machine_id}.local:8443/acp/v1"
  ws_endpoint: "wss://{machine_id}.local:8443/acp/stream"
  capabilities:
    - code-review
    - test-generation
    - refactoring

peers: []

security:
  default_auth_type: "signed-token"
  token_ttl_seconds: 3600
  require_https: true
  min_tls_version: "1.3"

retry:
  max_attempts: 3
  initial_backoff_ms: 1000
  max_backoff_ms: 30000
  backoff_multiplier: 2.0

timeouts:
  hop_ack_ms: 5000
  process_ack_ms: 300000
  stream_init_ms: 10000
  idle_close_ms: 60000
"#;

/// Render [`ACP_CONFIG_TEMPLATE`] for a new agent.
///
/// `machine_id` defaults to this host's name.
#[must_use]
pub fn generate_config(agent_id: &str, machine_id: Option<&str>) -> String {
    let machine_id = machine_id.map_or_else(local_machine_id, String::from);

    ACP_CONFIG_TEMPLATE
        .replace("{agent_id}", agent_id)
        .replace("{machine_id}", &machine_id)
        .replace("{updated_at}", &chrono::Utc::now().to_rfc3339())
}
