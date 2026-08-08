//! ACP Agent error types

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
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

    #[error("IO error: {0}")]
    Io(String),

    #[error("Agent error: {0}")]
    Agent(String),
}
