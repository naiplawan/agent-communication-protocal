//! ACP Relay — a rendezvous point for agents that cannot reach each other directly.
//!
//! Agents register their public endpoint ([`app::register`]) and the relay either
//! pushes messages straight to that endpoint or, when the push fails, holds them
//! in `SQLite` ([`store::Store`]) for the recipient to collect by polling. Every
//! request is authenticated with an HMAC token ([`security::verify_token`]).
//!
//! # Modules
//! - [`app`] — HTTP handlers and the router
//! - [`models`] — wire types shared by the handlers and the store
//! - [`security`] — token verification
//! - [`store`] — SQLite-backed message and peer registry

pub mod app;
pub mod models;
pub mod security;
pub mod store;
