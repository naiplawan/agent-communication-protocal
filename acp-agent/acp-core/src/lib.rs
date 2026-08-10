//! ACP Core — Agent Communication Protocol core types
//!
//! The protocol moves a [`protocol::Message`] between agents: an
//! [`protocol::Envelope`] carrying routing metadata plus an opaque JSON payload.
//! Each hop signs its request with an HMAC token bound to the message
//! ([`security::create_token`]), and appends itself to the envelope's reply path
//! so an answer can unwind the same route ([`protocol::reply_envelope`]).
//!
//! # Example
//! ```
//! use acp_core::protocol::{build_envelope, Intent, NewEnvelope, Origin, Priority};
//! use acp_core::protocol::{new_corr_id, new_msg_id};
//!
//! let envelope = build_envelope(NewEnvelope {
//!     msg_id: new_msg_id(),
//!     corr_id: new_corr_id(),
//!     origin: Origin::default(),
//!     sender: ("agent-alpha", "laptop-1"),
//!     recipient: ("agent-beta", "server-1"),
//!     intent: Intent::Delegate,
//!     reply_to_path: Some(vec!["agent-alpha@laptop-1".to_string()]),
//!     reply_to_ws_endpoint: None,
//!     hops_max: 10,
//!     content_type: "application/json",
//!     priority: Priority::Normal,
//!     deadline: None,
//! });
//!
//! assert_eq!(envelope.recipient.to_str(), "agent-beta@server-1");
//! ```
//!
//! # Modules
//! - [`protocol`] — message envelope, ULID generation, path building, streaming frames
//! - [`security`] — HMAC-SHA256 signed tokens, mTLS helpers
//! - [`transport`] — HTTP client with retry, WebSocket client
//! - [`config`] — YAML config loading, peer management
//! - [`chp`] — Context Handoff Protocol (rich task delegation)
//! - [`error`] — crate-wide error union

pub mod chp;
pub mod config;
pub mod error;
pub mod protocol;
pub mod security;
pub mod transport;

pub use error::Error;
