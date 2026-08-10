//! ACP Agent error types
//!
//! Library code in this crate returns these; the CLI in `main.rs` is the only
//! place that reaches for `anyhow`.

use acp_core::config::ConfigError;
use acp_core::protocol::{HopsExceededError, ReplyPathEmptyError};
use acp_core::transport::TransportError;
use thiserror::Error;

/// Why an agent operation did not complete.
#[derive(Error, Debug)]
pub enum AgentError {
    /// The target is not listed in `acp-peers.yaml`.
    #[error("Unknown peer: {0}")]
    UnknownPeer(String),

    /// A reply's next hop resolved to an address with no matching peer.
    #[error("Peer not found for {context} recipient: {addr}")]
    UnroutableReply {
        /// Which send path failed — `"reply"`, `"error"`, or `"stream"`.
        context: &'static str,
        /// Address that could not be resolved.
        addr: String,
    },

    /// The peer config could not be loaded or resolved.
    #[error("Config error: {0}")]
    Config(#[from] ConfigError),

    /// A forward would have pushed the chain past its hop ceiling.
    #[error("Hops exceeded: {0}")]
    HopsExceeded(#[from] HopsExceededError),

    /// A reply had nowhere to go.
    #[error("Reply path empty: {0}")]
    ReplyPathEmpty(#[from] ReplyPathEmptyError),

    /// The peer could not be reached, or refused the message.
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),

    /// A payload could not be serialized or deserialized.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// The process could not bind a socket or wait on a signal.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Why the signaling client could not talk to the relay.
#[derive(Error, Debug)]
pub enum SignalingError {
    /// A required environment variable is unset.
    #[error("{0} must be set")]
    MissingEnv(&'static str),

    /// The relay could not be reached.
    #[error("Relay request failed: {0}")]
    Request(#[from] reqwest::Error),

    /// The relay answered with a non-success status.
    #[error("Relay rejected {operation}: {status} {body}")]
    Rejected {
        /// Which call was refused — `"registration"`, `"send"`, `"poll"`, `"ack"`.
        operation: &'static str,
        /// Status the relay returned.
        status: u16,
        /// Response body, when the relay sent one.
        body: String,
    },
}
