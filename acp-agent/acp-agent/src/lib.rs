//! ACP Agent — a full ACP participant with an HTTP server, relay signaling, and
//! a handler registry.
//!
//! [`ACPAgent`] owns the peer config and the signed HTTP client; register
//! handlers on it with [`ACPAgent::on_delegate`] and friends, then serve it with
//! [`server::start_server`]. For agents behind NAT, [`signaling`] registers the
//! endpoint with a relay and polls it instead of accepting inbound connections.
//!
//! # Modules
//! - [`agent`] — the agent itself: send, delegate, reply, stream
//! - [`server`] — Axum router exposing the ACP endpoints
//! - [`signaling`] — relay registration and polling for NAT traversal
//! - [`error`] — this crate's error types

pub mod agent;
pub mod error;
pub mod server;
pub mod signaling;

pub use agent::ACPAgent;
pub use error::{AgentError, SignalingError};
