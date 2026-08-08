//! ACP Core error types

use thiserror::Error;

// ---------------------------------------------------------------------------
// Top-level error type
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum Error {
    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Security error: {0}")]
    Security(String),

    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("CHP error: {0}")]
    Chp(String),

    #[error("Hops exceeded: {0}")]
    HopsExceeded(String),

    #[error("Reply path empty: {0}")]
    ReplyPathEmpty(String),

    #[error("IO error: {0}")]
    Io(String),
}

impl From<crate::protocol::HopsExceededError> for Error {
    fn from(e: crate::protocol::HopsExceededError) -> Self {
        Error::HopsExceeded(e.to_string())
    }
}

impl From<crate::protocol::ReplyPathEmptyError> for Error {
    fn from(e: crate::protocol::ReplyPathEmptyError) -> Self {
        Error::ReplyPathEmpty(e.to_string())
    }
}

impl From<crate::config::ConfigError> for Error {
    fn from(e: crate::config::ConfigError) -> Self {
        Error::Config(e.to_string())
    }
}

impl From<crate::security::TokenError> for Error {
    fn from(e: crate::security::TokenError) -> Self {
        Error::Security(e.to_string())
    }
}
