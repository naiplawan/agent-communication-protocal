//! ACP Core — Agent Communication Protocol core types
//!
//! Core modules:
//! - `protocol` — message envelope, ULID generation, path building, streaming frames
//! - `security` — HMAC-SHA256 signed tokens, mTLS helpers
//! - `transport` — HTTP client with retry, WebSocket client
//! - `config` — YAML config loading, peer management
//! - `chp` — Context Handoff Protocol (rich task delegation)
//! - `error` — error types

pub mod protocol;
pub mod security;
pub mod transport;
pub mod config;
pub mod chp;
pub mod error;

// Re-export commonly used types
pub use protocol::*;
pub use security::*;
pub use config::*;
pub use chp::*;
pub use error::*;
