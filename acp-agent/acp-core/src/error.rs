//! ACP Core error types
//!
//! Each module owns a precise error type — [`crate::config::ConfigError`],
//! [`crate::security::TokenError`], [`crate::transport::TransportError`]. [`enum@Error`]
//! is the crate-wide union for callers that handle any of them at one boundary.

use thiserror::Error;

use crate::config::ConfigError;
use crate::protocol::{HopsExceededError, ReplyPathEmptyError};
use crate::security::TokenError;
use crate::transport::TransportError;

/// Any failure originating inside `acp-core`.
#[derive(Error, Debug)]
pub enum Error {
    /// A message could not be built, parsed, or routed.
    #[error("Protocol error: {0}")]
    Protocol(String),

    /// A token could not be minted or verified.
    #[error("Security error: {0}")]
    Security(#[from] TokenError),

    /// A peer could not be reached, or refused a request.
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),

    /// The peer config could not be loaded.
    #[error("Config error: {0}")]
    Config(#[from] ConfigError),

    /// A context handoff could not be built or read.
    #[error("CHP error: {0}")]
    Chp(String),

    /// A forward would have pushed the chain past its hop ceiling.
    #[error("Hops exceeded: {0}")]
    HopsExceeded(#[from] HopsExceededError),

    /// A reply had nowhere to go.
    #[error("Reply path empty: {0}")]
    ReplyPathEmpty(#[from] ReplyPathEmptyError),

    /// A file could not be read or written.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
