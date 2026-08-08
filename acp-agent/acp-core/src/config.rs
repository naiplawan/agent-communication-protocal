//! ACP Config — peer configuration loaded from acp-peers.yaml

use crate::security::PeerAuth;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ThisAgent {
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_endpoint: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Peer {
    pub agent_id: String,
    pub machine_id: String,
    pub http_endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_endpoint: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<PeerAuth>,
}

impl Peer {
    pub fn addr(&self) -> String {
        format!("{}@{}", self.agent_id, self.machine_id)
    }

    pub fn machine_id_ref(&self) -> &str {
        &self.machine_id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SecurityConfig {
    #[serde(default = "default_auth_type")]
    pub default_auth_type: String,
    #[serde(default = "default_token_ttl")]
    pub token_ttl_seconds: u64,
    #[serde(default = "default_require_https")]
    pub require_https: bool,
    #[serde(default = "default_min_tls")]
    pub min_tls_version: String,
}

fn default_auth_type() -> String {
    "signed-token".to_string()
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RetryConfig {
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_initial_backoff")]
    pub initial_backoff_ms: u64,
    #[serde(default = "default_max_backoff")]
    pub max_backoff_ms: u64,
    #[serde(default = "default_backoff_multiplier")]
    pub backoff_multiplier: f64,
}

fn default_max_attempts() -> u32 { 3 }
fn default_initial_backoff() -> u64 { 1000 }
fn default_max_backoff() -> u64 { 30000 }
fn default_backoff_multiplier() -> f64 { 2.0 }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TimeoutsConfig {
    #[serde(default = "default_hop_ack")]
    pub hop_ack_ms: u64,
    #[serde(default = "default_process_ack")]
    pub process_ack_ms: u64,
    #[serde(default = "default_stream_init")]
    pub stream_init_ms: u64,
    #[serde(default = "default_idle_close")]
    pub idle_close_ms: u64,
}

fn default_hop_ack() -> u64 { 5000 }
fn default_process_ack() -> u64 { 300000 }
fn default_stream_init() -> u64 { 10000 }
fn default_idle_close() -> u64 { 60000 }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ACPConfig {
    #[serde(default)]
    pub config_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub this_agent: Option<ThisAgent>,
    #[serde(default)]
    pub peers: Vec<Peer>,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub timeouts: TimeoutsConfig,
}

impl ACPConfig {
    pub fn get_peer(&self, agent_id: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.agent_id == agent_id)
    }

    pub fn get_peer_by_addr(&self, addr: &str) -> Option<&Peer> {
        let (agent_id, machine_id) = addr.rsplit_once('@').unwrap_or((addr, ""));
        self.peers
            .iter()
            .find(|p| p.agent_id == agent_id && p.machine_id.as_str() == machine_id)
    }
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

const CONFIG_ENV: &str = "ACP_PEERS_PATH";
const DEFAULT_CONFIG_NAME: &str = "acp-peers.yaml";

/// Load ACP peer config from a YAML file.
///
/// Resolution order:
/// 1. ACP_PEERS_PATH env var
/// 2. ./acp-peers.yaml (cwd)
/// 3. ~/.acp/acp-peers.yaml
/// 4. /etc/acp/acp-peers.yaml
pub fn load_config(path: Option<&str>) -> Result<ACPConfig, ConfigError> {
    let candidates: Vec<PathBuf> = if let Some(p) = path {
        vec![PathBuf::from(p)]
    } else if let Ok(p) = env::var(CONFIG_ENV) {
        vec![PathBuf::from(p)]
    } else {
        let mut v = Vec::new();
        v.push(PathBuf::from(".").join(DEFAULT_CONFIG_NAME));
        if let Some(home) = dirs::home_dir() {
            v.push(home.join(".acp").join(DEFAULT_CONFIG_NAME));
        }
        v.push(PathBuf::from("/etc/acp").join(DEFAULT_CONFIG_NAME));
        v
    };

    for candidate in &candidates {
        if candidate.exists() {
            return load_from_path(candidate);
        }
    }

    Err(ConfigError::NotFound(
        candidates.iter().map(|p| p.to_string_lossy().to_string()).collect(),
    ))
}

/// Load config from a specific path
pub fn load_from_path(path: &PathBuf) -> Result<ACPConfig, ConfigError> {
    let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Io(e.to_string()))?;

    let cfg: ACPConfig = serde_yaml::from_str(&content)
        .map_err(|e| ConfigError::Parse(e.to_string()))?;

    Ok(cfg)
}

/// Resolve this_agent, inferring machine_id from hostname if not set
pub fn resolve_this_agent(config: &ACPConfig) -> Result<ThisAgent, ConfigError> {
    let ta = config.this_agent.as_ref().ok_or(ConfigError::MissingThisAgent)?;

    let mut resolved = ta.clone();
    if resolved.machine_id.is_none() {
        resolved.machine_id = Some(hostname::get()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string()));
    }
    if resolved.http_endpoint.is_none() {
        return Err(ConfigError::MissingThisAgent);
    }
    Ok(resolved)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    NotFound(Vec<String>),
    Io(String),
    Parse(String),
    MissingThisAgent,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::NotFound(paths) => {
                write!(f, "No ACP config found. Checked: {:?}", paths)
            }
            ConfigError::Io(s) => write!(f, "IO error: {}", s),
            ConfigError::Parse(s) => write!(f, "Parse error: {}", s),
            ConfigError::MissingThisAgent => write!(f, "this_agent not set in config"),
        }
    }
}

impl std::error::Error for ConfigError {}

// ---------------------------------------------------------------------------
// Config template generator
// ---------------------------------------------------------------------------

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

/// Generate a config string for a new agent
pub fn generate_config(agent_id: &str, machine_id: Option<&str>) -> String {
    let machine_id = machine_id
        .map(String::from)
        .unwrap_or_else(|| {
            hostname::get()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        });

    ACP_CONFIG_TEMPLATE
        .replace("{agent_id}", agent_id)
        .replace("{machine_id}", &machine_id)
        .replace("{updated_at}", &chrono::Utc::now().to_rfc3339())
}
