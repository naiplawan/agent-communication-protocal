//! ACP Relay HTTP handlers
//!
//! Two delivery modes: when the recipient is registered *and* reachable the
//! relay pushes the message to its endpoint; otherwise it brokers — the message
//! is stored and the recipient collects it by polling.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{AppendHeaders, IntoResponse, Response};
use axum::routing::{get, options, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::models::{HopTraceEntry, Hops, Peer, SendRequest, SendResponse, TokenClaims};
use crate::security::{extract_agent_id, secret_bytes, verify_token, TokenError};
use crate::store::Store;

type HmacSha256 = Hmac<sha2::Sha256>;

/// Machine ID the relay signs its own tokens with.
const RELAY_MACHINE_ID: &str = "relay";

/// Lifetime of tokens the relay mints for forwarding.
const TOKEN_TTL_SECONDS: i64 = 3600;

/// How long the relay waits for a push before treating it as failed.
const FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Hop ceiling stamped onto a message that arrives without hop tracking.
const DEFAULT_MAX_HOPS: u32 = 10;

/// Current ACP wire-protocol version.
const PROTOCOL_VERSION: &str = "1.1";

/// Wire versions accepted during initialization, newest first.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["1.1", "1.0"];

/// Body of `POST /acp/v1/initialize`.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InitializeRequest {
    /// Protocol versions the caller can speak, newest first.
    #[serde(default)]
    protocol_versions: Vec<String>,
    /// Features the caller would like to use.
    #[serde(default)]
    capabilities: Vec<String>,
}

/// Everything a handler needs: the store, the signing secret, and the live feed.
#[derive(Clone)]
pub struct AppState {
    /// Message store and peer registry.
    pub store: Store,
    /// Secret every ACP token is signed with.
    pub shared_secret: String,
    /// The relay's own agent ID, used as a token audience.
    pub this_agent_id: String,
    /// Fan-out channel backing `/acp/stream/live`.
    pub broadcast_tx: broadcast::Sender<String>,
}

type SharedState = Arc<AppState>;

const CORS_HEADERS: [(axum::http::HeaderName, &str); 4] = [
    (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
    (
        axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
        "GET, POST, OPTIONS",
    ),
    (
        axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS,
        "Authorization, Content-Type",
    ),
    (axum::http::header::ACCESS_CONTROL_MAX_AGE, "86400"),
];

fn with_cors(body: impl IntoResponse) -> Response {
    let mut resp = body.into_response();
    let headers = resp.headers_mut();
    for (name, value) in &CORS_HEADERS {
        if let Ok(val) = value.parse() {
            headers.insert(name.clone(), val);
        }
    }
    resp
}

/// Verify the `Authorization` header and return the token's claims.
fn verify_auth(headers: &HeaderMap, secret: &str) -> Result<TokenClaims, &'static str> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or("Missing Authorization header")?;

    let token = auth
        .strip_prefix("ACP-Token ")
        .ok_or("Invalid authorization header format")?;

    verify_token(token, secret).map_err(|e| {
        warn!("Token verification failed: {e}");
        match e {
            TokenError::Expired => "Token expired",
            _ => "Invalid token",
        }
    })
}

/// Why a request was turned away before any handler logic ran.
enum Rejection {
    /// The token was missing, malformed, or not signed with the shared secret.
    Unauthorized(&'static str),
    /// The token was valid but minted for a different audience.
    Forbidden(String),
}

impl IntoResponse for Rejection {
    fn into_response(self) -> Response {
        match self {
            Rejection::Unauthorized(msg) => auth_error(msg),
            Rejection::Forbidden(msg) => forbidden(&msg),
        }
    }
}

/// Run the shared auth + audience check, returning the caller's agent ID.
///
/// `allowed` lists the audiences this endpoint accepts. Most only accept the
/// relay itself; `send_message` also accepts the ultimate recipient.
fn authorize(
    headers: &HeaderMap,
    state: &AppState,
    allowed: &[&str],
) -> Result<TokenClaims, Rejection> {
    let claims = verify_auth(headers, &state.shared_secret).map_err(Rejection::Unauthorized)?;
    check_audience(&claims, allowed).map_err(Rejection::Forbidden)?;
    Ok(claims)
}

fn authorize_observability(headers: &HeaderMap, state: &AppState) -> Option<Response> {
    if std::env::var("ACP_PUBLIC_DEBUG")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        warn!("ACP_PUBLIC_DEBUG is enabled; exposing relay diagnostics publicly");
        return None;
    }
    authorize(headers, state, &[&state.this_agent_id])
        .err()
        .map(IntoResponse::into_response)
}

// ---- Health ----

/// `GET /health` — liveness, unauthenticated.
pub async fn health() -> Response {
    with_cors(Json(
        serde_json::json!({ "status": "ok", "agent": "acp-relay" }),
    ))
}

// ---- Register ----

/// `POST /acp/v1/agents/register` — record where an agent can be pushed to.
pub async fn register(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(peer): Json<Peer>,
) -> Response {
    let claims = match authorize(&headers, &state, &[&state.this_agent_id]) {
        Ok(claims) => claims,
        Err(rejection) => return rejection.into_response(),
    };
    let issuer = agent_id_from_iss(&claims.iss);
    // A registration rewrites where the relay forwards an agent's messages, so it
    // may only be made in the registrant's own name — otherwise any holder of the
    // shared secret could point another agent's traffic at an endpoint it controls.
    let expected_issuer = format!("{}@{}", peer.agent_id, peer.machine_id);
    if issuer != peer.agent_id || claims.iss != expected_issuer {
        warn!("[REG] {issuer} attempted to register as {}", peer.agent_id);
        return forbidden(&format!(
            "Token issued by {issuer} cannot register {}",
            peer.agent_id
        ));
    }
    if !valid_endpoint(&peer.http_endpoint, &["http", "https"])
        || peer
            .ws_endpoint
            .as_deref()
            .is_some_and(|endpoint| !valid_endpoint(endpoint, &["ws", "wss"]))
    {
        return error_resp("INVALID_ENDPOINT", StatusCode::BAD_REQUEST);
    }

    info!("[REG] Registering peer: {}", peer.agent_id);
    if let Err(e) = state.store.register_peer(&peer) {
        error!("[REG] Failed to register peer: {e}");
        return error_resp("DB_ERROR", StatusCode::INTERNAL_SERVER_ERROR);
    }

    let peers = state.store.get_peers(false).unwrap_or_default();
    with_cors(Json(serde_json::json!({
        "status": "registered",
        "agent_id": peer.agent_id,
        "peers": peers,
        "this_agent": { "agent_id": state.this_agent_id },
    })))
}

// ---- Peers ----

/// `GET /acp/v1/peers` — the agents currently registered and unexpired.
pub async fn get_peers(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Err(rejection) = authorize(&headers, &state, &[&state.this_agent_id]) {
        return rejection.into_response();
    }
    let peers = state.store.get_peers(false).unwrap_or_default();
    with_cors(Json(serde_json::json!({ "peers": peers })))
}

// ---- Send message ----

/// `POST /acp/v1/messages/send` — deliver a message, by push or by broker.
pub async fn send_message(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<SendRequest>,
) -> Response {
    let envelope = &body.envelope;
    let msg_id = &envelope.msg_id;
    let recipient_agent = &envelope.recipient.agent_id;

    // Unlike the relay's own endpoints, a send token may be addressed to the
    // ultimate recipient — the relay is a forwarder on that path, not the target.
    let claims = match authorize(&headers, &state, &[&state.this_agent_id, recipient_agent]) {
        Ok(claims) => claims,
        Err(rejection) => return rejection.into_response(),
    };
    let expected_sender = format!(
        "{}@{}",
        envelope.sender.agent_id,
        envelope.sender.machine_id.as_deref().unwrap_or_default()
    );
    if claims.iss != expected_sender {
        return forbidden("Token issuer does not match envelope sender");
    }

    info!(
        "[SEND] msg_id={} -> {recipient_agent}",
        &msg_id[..8.min(msg_id.len())]
    );

    if recipient_agent == &state.this_agent_id {
        return deliver_locally(&state, &body);
    }

    // A peer whose endpoint has already failed a push is left registered but is
    // no longer a forward target, so poll-only agents don't cost a timeout per
    // message. They collect the same message from the broker instead.
    let peers = state.store.get_peers(false).unwrap_or_default();
    let Some(peer) = peers
        .iter()
        .find(|p| p.agent_id == *recipient_agent && p.reachable)
    else {
        return broker(&state, &body);
    };

    forward(&state, &body, peer).await
}

/// Store a message addressed to the relay itself and announce it on the live feed.
fn deliver_locally(state: &AppState, body: &SendRequest) -> Response {
    let envelope = &body.envelope;
    let msg_id = &envelope.msg_id;

    if let Err(e) = state.store.put(msg_id, envelope, &body.payload) {
        error!("[SEND] Local put failed: {e}");
    }
    state.store.update_status(msg_id, "accepted").ok();

    broadcast(
        state,
        &serde_json::json!({
            "type": "message",
            "data": {
                "msg_id": msg_id,
                "intent": envelope.intent,
                "sender_agent": envelope.sender.agent_id,
                "recipient_agent": envelope.recipient.agent_id,
                "payload": body.payload,
            }
        }),
    );

    with_cors(send_ok(msg_id, "accepted", None))
}

/// Hold a message for the recipient to collect by polling.
fn broker(state: &AppState, body: &SendRequest) -> Response {
    let msg_id = &body.envelope.msg_id;
    if let Err(e) = state.store.put(msg_id, &body.envelope, &body.payload) {
        error!("[SEND] Broker put failed: {e}");
        return error_resp("DB_ERROR", StatusCode::INTERNAL_SERVER_ERROR);
    }
    info!(
        "[SEND] Brokered msg {msg_id} for {}",
        body.envelope.recipient.agent_id
    );
    with_cors(send_ok(msg_id, "brokered", None))
}

/// Push a message to a reachable peer, falling back to brokering when it fails.
async fn forward(state: &AppState, body: &SendRequest, peer: &Peer) -> Response {
    let envelope = &body.envelope;
    let msg_id = &envelope.msg_id;
    let current_hops = envelope.hops.as_ref().map_or(0, |hops| hops.count);
    let max_hops = envelope
        .hops
        .as_ref()
        .map_or(DEFAULT_MAX_HOPS, |hops| hops.max);
    if max_hops == 0 || current_hops >= max_hops {
        return error_resp("HOP_LIMIT_EXCEEDED", StatusCode::BAD_REQUEST);
    }

    let mut new_envelope = envelope.clone();
    new_envelope.sender.agent_id = state.this_agent_id.clone();
    new_envelope.sender.machine_id = Some(RELAY_MACHINE_ID.to_string());
    let trace = HopTraceEntry::now(&state.this_agent_id, RELAY_MACHINE_ID);
    match &mut new_envelope.hops {
        Some(hops) => {
            hops.count += 1;
            hops.trace.push(trace);
        }
        None => {
            new_envelope.hops = Some(Hops {
                count: 1,
                max: DEFAULT_MAX_HOPS,
                trace: vec![trace],
            });
        }
    }

    let forward_token = create_token(
        &state.this_agent_id,
        RELAY_MACHINE_ID,
        &envelope.recipient.agent_id,
        envelope.recipient.machine_id.as_deref().unwrap_or(""),
        msg_id,
        &state.shared_secret,
    );

    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return error_resp(
                &format!("FORWARD_CLIENT_ERROR: {error}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let result = client
        .post(format!("{}/acp/v1/relay/forward", peer.http_endpoint))
        .json(&serde_json::json!({ "envelope": new_envelope, "payload": body.payload }))
        .header("Authorization", format!("ACP-Token {forward_token}"))
        .timeout(FORWARD_TIMEOUT)
        .send()
        .await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            state.store.put(msg_id, envelope, &body.payload).ok();
            // The push landed, so the message is delivered rather than awaiting a
            // poll. It stays pollable until acked, per at-least-once delivery.
            state.store.update_status(msg_id, "delivered").ok();
            info!("[SEND] Forwarded {msg_id} to {}", peer.http_endpoint);
            with_cors(send_ok(msg_id, "forwarded", Some(&peer.http_endpoint)))
        }
        Ok(resp) => {
            let status = resp.status();
            warn!(
                "[SEND] Forward failed {status}: {}",
                resp.text().await.unwrap_or_default()
            );
            broker(state, body)
        }
        Err(e) if is_connection_failure(&e) => {
            state
                .store
                .mark_peer_unreachable(&envelope.recipient.agent_id)
                .ok();
            warn!(
                "[SEND] Peer {} unreachable ({e}) - re-brokering",
                envelope.recipient.agent_id
            );
            broker(state, body)
        }
        Err(e) => {
            error!("[SEND] Forward error: {e}");
            error_resp("FORWARD_ERROR", StatusCode::BAD_GATEWAY)
        }
    }
}

/// Whether a push failed because the endpoint was not answering.
///
/// `reqwest` folds every connect-level failure into an opaque "error sending
/// request", so the kind flags are checked first and the message text is only a
/// fallback for the cases they do not cover.
fn is_connection_failure(error: &reqwest::Error) -> bool {
    if error.is_connect() || error.is_timeout() {
        return true;
    }
    let text = error.to_string().to_lowercase();
    text.contains("connection refused")
        || text.contains("connection reset")
        || text.contains("connection timed out")
        || text.contains("error sending request")
}

// ---- Poll pending ----

/// `GET /acp/v1/messages/pending` — collect the messages held for the caller.
pub async fn get_pending(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    let claims = match authorize(&headers, &state, &[&state.this_agent_id]) {
        Ok(claims) => claims,
        Err(rejection) => return rejection.into_response(),
    };
    let agent_id = agent_id_from_iss(&claims.iss);

    // Polling is the liveness signal for agents that don't accept pushes; without
    // this their last_seen_at only moves on re-registration and they read as stale.
    state.store.touch_peer(&agent_id).ok();
    let messages = state.store.get_all_pending(&agent_id).unwrap_or_default();
    info!("[PENDING] {} messages for {agent_id}", messages.len());
    with_cors(Json(
        serde_json::json!({ "count": messages.len(), "messages": messages }),
    ))
}

// ---- Acknowledge message ----

/// `POST /acp/v1/messages/{msg_id}/ack` — record that a message landed.
pub async fn acknowledge_message(
    State(state): State<SharedState>,
    Path(msg_id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let claims = match authorize(&headers, &state, &[&state.this_agent_id]) {
        Ok(claims) => claims,
        Err(rejection) => return rejection.into_response(),
    };
    let issuer = agent_id_from_iss(&claims.iss);
    let Some((_stored_sender, stored_recipient)) =
        state.store.get_message_addresses(&msg_id).ok().flatten()
    else {
        return error_resp("NOT_FOUND", StatusCode::NOT_FOUND);
    };
    if stored_recipient != issuer {
        return forbidden("Only the message recipient may acknowledge it");
    }

    // The body is read leniently: callers send differing shapes (`ack_type` +
    // `received`, or the spec's `ack_id`/`processed`/`stream_available`), and some
    // send none at all. A processed ack completes the message; anything else only
    // confirms the hop.
    let ack: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let processed = ack
        .get("processed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let ack_id = ack.get("ack_id").and_then(|v| v.as_str());
    let status = if processed {
        "completed"
    } else {
        "acknowledged"
    };

    info!("[ACK] {msg_id} -> {status} (from {issuer})");

    match state.store.update_status(&msg_id, status) {
        Ok(()) => {
            broadcast(
                &state,
                &serde_json::json!({
                    "type": "status",
                    "data": { "msg_id": msg_id, "status": status }
                }),
            );
            with_cors(Json(serde_json::json!({
                "msg_id": msg_id,
                "status": status,
                "ack_id": ack_id,
                "recorded": true,
            })))
        }
        Err(error) => error_resp(
            &format!("ACK_ERROR: {error}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

// ---- Message status ----

/// `GET /acp/v1/messages/{msg_id}/status` — where one message stands.
pub async fn message_status(
    State(state): State<SharedState>,
    Path(msg_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(rejection) = authorize(&headers, &state, &[&state.this_agent_id]) {
        return rejection.into_response();
    }

    match state.store.get_status(&msg_id) {
        Ok(Some((status, updated_at))) => with_cors(Json(serde_json::json!({
            "msg_id": msg_id,
            "status": status,
            "delivered_at": iso_from_epoch(updated_at),
        }))),
        Ok(None) => error_resp("NOT_FOUND", StatusCode::NOT_FOUND),
        Err(error) => error_resp(
            &format!("STATUS_ERROR: {error}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

// ---- Capabilities ----

/// `GET /acp/v1/capabilities` — what the relay can speak.
///
/// Unauthenticated, like `/health`: it carries no message or peer data, and a
/// client may need it to negotiate before it can address a token correctly.
pub async fn capabilities(State(state): State<SharedState>) -> Response {
    with_cors(Json(serde_json::json!({
        "protocol_version": PROTOCOL_VERSION,
        "agent_id": state.this_agent_id,
        "machine_id": RELAY_MACHINE_ID,
        "role": "relay",
        "capabilities": ["relay", "registry", "broker"],
        "intents": ["delegate", "reply", "ack", "error"],
        "content_types": ["application/json"],
        "auth": ["signed-token"],
    })))
}

/// `POST /acp/v1/initialize` — authenticate and negotiate protocol features.
pub async fn initialize(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<InitializeRequest>,
) -> Response {
    let claims = match authorize(&headers, &state, &[&state.this_agent_id]) {
        Ok(claims) => claims,
        Err(rejection) => return rejection.into_response(),
    };
    if claims.msg_id != "initialize" {
        return forbidden("Initialization tokens must be bound to initialize");
    }

    let protocol_version = SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .find(|version| {
            body.protocol_versions.is_empty()
                || body
                    .protocol_versions
                    .iter()
                    .any(|candidate| candidate == **version)
        })
        .copied();
    let Some(protocol_version) = protocol_version else {
        return error_resp("UNSUPPORTED_PROTOCOL_VERSION", StatusCode::BAD_REQUEST);
    };

    let features = ["session-context", "run-context"];
    let accepted_capabilities: Vec<&str> = body
        .capabilities
        .iter()
        .map(String::as_str)
        .filter(|capability| features.contains(capability))
        .collect();

    with_cors(Json(serde_json::json!({
        "protocol_version": protocol_version,
        "server_protocol_version": PROTOCOL_VERSION,
        "supported_protocol_versions": SUPPORTED_PROTOCOL_VERSIONS,
        "role": "relay",
        "agent_id": state.this_agent_id,
        "machine_id": RELAY_MACHINE_ID,
        "capabilities": ["relay", "registry", "broker"],
        "accepted_capabilities": accepted_capabilities,
        "features": features,
        "intents": ["delegate", "reply", "ack", "error"],
        "content_types": ["application/json"],
        "auth": ["signed-token"],
    })))
}

// ---- Debug messages ----

/// `GET /acp/v1/debug/messages` — recent traffic, for the dashboard.
pub async fn debug_messages(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Some(response) = authorize_observability(&headers, &state) {
        return response;
    }
    let messages = state.store.get_all_messages().unwrap_or_default();
    with_cors(Json(serde_json::json!({ "messages": messages })))
}

// ---- CORS preflight ----

/// `OPTIONS /<path>` — CORS preflight.
pub async fn cors_preflight() -> impl IntoResponse {
    AppendHeaders(CORS_HEADERS.to_vec())
}

// ---- WebSocket live stream ----

/// `GET /acp/stream/live` — subscribe to message and status events.
pub async fn ws_live(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = authorize_observability(&headers, &state) {
        return response;
    }
    ws.on_upgrade(move |socket| ws_handler(socket, state))
}

async fn ws_handler(ws: WebSocket, state: SharedState) {
    let (mut write, mut read) = ws.split();
    let mut rx = state.broadcast_tx.subscribe();

    // Opening frame: how much traffic the store already holds.
    if let Ok(messages) = state.store.get_all_messages() {
        let init = serde_json::json!({ "type": "init", "count": messages.len() });
        let _ = write.send(WsMessage::Text(init.to_string().into())).await;
    }

    loop {
        tokio::select! {
            biased;
            event = rx.recv() => {
                let Ok(event) = event else { break };
                if write.send(WsMessage::Text(event.into())).await.is_err() {
                    break;
                }
            }
            item = read.next() => {
                match item {
                    Some(Ok(WsMessage::Ping(data))) => {
                        let _ = write.send(WsMessage::Pong(data)).await;
                    }
                    Some(Ok(WsMessage::Close(_)) | Err(_)) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// Publish an event to every live subscriber. No subscribers is not a failure.
fn broadcast(state: &AppState, event: &serde_json::Value) {
    let _ = state.broadcast_tx.send(event.to_string());
}

// ---- Helpers ----

fn send_ok(msg_id: &str, status: &str, next_hop: Option<&str>) -> Json<SendResponse> {
    Json(SendResponse {
        msg_id: msg_id.to_string(),
        status: status.to_string(),
        next_hop: next_hop.map(String::from),
        error: None,
    })
}

fn error_resp(error: &str, status: StatusCode) -> Response {
    let mut resp = Json(serde_json::json!({ "error": error })).into_response();
    *resp.status_mut() = status;
    with_cors(resp)
}

fn auth_error(msg: &str) -> Response {
    let mut resp =
        Json(serde_json::json!({ "error": "UNAUTHORIZED", "message": msg })).into_response();
    *resp.status_mut() = StatusCode::UNAUTHORIZED;
    with_cors(resp)
}

fn forbidden(message: &str) -> Response {
    let mut resp =
        Json(serde_json::json!({ "error": "FORBIDDEN", "message": message })).into_response();
    *resp.status_mut() = StatusCode::FORBIDDEN;
    with_cors(resp)
}

fn agent_id_from_iss(iss: &str) -> String {
    extract_agent_id(iss)
}

fn valid_endpoint(endpoint: &str, schemes: &[&str]) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    schemes.contains(&url.scheme())
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

/// Confirm a token was minted for one of `allowed` audiences.
///
/// Only the `agent_id` half of `aud` is compared. Peers derive the machine half
/// from their own config entry for the relay, which deployments name freely, so
/// requiring an exact match would reject correctly-signed tokens. Comparing the
/// agent half still stops a token minted to address one agent from being replayed
/// against another's endpoints.
fn check_audience(claims: &TokenClaims, allowed: &[&str]) -> Result<(), String> {
    let aud_agent = extract_agent_id(&claims.sub);
    if allowed.contains(&aud_agent.as_str()) {
        return Ok(());
    }
    Err(format!(
        "Token audience {aud_agent} is not valid here (expected one of: {})",
        allowed.join(", ")
    ))
}

/// Widest `f64` timestamps accepted by [`iso_from_epoch`].
///
/// Comfortably inside `i64` range, so the cast below cannot overflow. Anything
/// beyond this is malformed data rather than a date — the year 285 million sits
/// around 9.0e15.
const EPOCH_SECS_MIN: f64 = -9.0e18;
const EPOCH_SECS_MAX: f64 = 9.0e18;

/// Render an `f64` Unix timestamp from the store as RFC 3339.
///
/// Out-of-range and non-finite input yields an empty string, as does any value
/// `chrono` refuses.
fn iso_from_epoch(secs: f64) -> String {
    // NaN fails this check too, so no separate finiteness test is needed.
    if !(EPOCH_SECS_MIN..=EPOCH_SECS_MAX).contains(&secs) {
        return String::new();
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "range-checked above; the fractional part is deliberately dropped"
    )]
    let secs = secs as i64;

    chrono::DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

// ---- Token creation ----

/// Mint a token binding `msg_id` to an issuer/audience pair.
///
/// Used for relay-signed forwards, and by the tests in [`crate::security`].
///
/// # Panics
/// Cannot panic in practice. The claims are a `serde_json::Value` built from
/// string keys, whose only documented serialization failure is a map with
/// non-string keys, and HMAC-SHA256 accepts a key of any length.
#[must_use]
pub fn create_token(
    issuer_agent_id: &str,
    issuer_machine_id: &str,
    audience_agent_id: &str,
    audience_machine_id: &str,
    msg_id: &str,
    secret: &str,
) -> String {
    let header_b64 = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"ACP"}"#);

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    let iso = |secs: i64| {
        chrono::DateTime::from_timestamp(secs, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default()
    };

    let payload = serde_json::json!({
        "iss": format!("{issuer_agent_id}@{issuer_machine_id}"),
        "aud": format!("{audience_agent_id}@{audience_machine_id}"),
        "exp": iso(now_secs + TOKEN_TTL_SECONDS),
        "iat": iso(now_secs),
        "msg_id": msg_id,
        "nonce": uuid::Uuid::new_v4().to_string(),
    });
    // A `serde_json::Value` built from string keys and string values always
    // serializes; the only documented failure is a map with non-string keys.
    let payload_bytes =
        serde_json::to_vec(&payload).expect("json! value with string keys always serializes");
    let payload_b64 = URL_SAFE_NO_PAD.encode(&payload_bytes);

    let mut mac = HmacSha256::new_from_slice(&secret_bytes(secret))
        .expect("HMAC-SHA256 accepts a key of any size");
    mac.update(format!("{header_b64}.{payload_b64}").as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes().as_slice());

    format!("{header_b64}.{payload_b64}.{sig_b64}")
}

// ---- Build router ----

/// Build the relay router around `state`.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/acp/v1/capabilities", get(capabilities))
        .route("/acp/v1/initialize", post(initialize))
        .route("/acp/v1/agents/register", post(register))
        .route("/acp/v1/messages/{msg_id}/status", get(message_status))
        .route("/acp/v1/peers", get(get_peers))
        .route("/acp/v1/messages/send", post(send_message))
        .route("/acp/v1/messages/pending", get(get_pending))
        .route("/acp/v1/messages/{msg_id}/ack", post(acknowledge_message))
        .route("/acp/v1/debug/messages", get(debug_messages))
        .route("/acp/stream/live", get(ws_live))
        .route("/{*path}", options(cors_preflight))
        .with_state(Arc::new(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims_for(aud: &str) -> TokenClaims {
        TokenClaims {
            iss: "agent-alpha@laptop-1".to_string(),
            sub: aud.to_string(),
            msg_id: "msg_1".to_string(),
            exp: 0,
        }
    }

    #[test]
    fn a_token_addressed_to_the_relay_is_accepted() {
        assert!(check_audience(&claims_for("acp-relay@relay"), &["acp-relay"]).is_ok());
    }

    #[test]
    fn a_token_addressed_to_another_agent_is_refused() {
        assert!(check_audience(&claims_for("agent-gamma@m"), &["acp-relay"]).is_err());
    }

    #[test]
    fn a_send_token_may_be_addressed_to_the_recipient() {
        let result = check_audience(
            &claims_for("agent-beta@server-1"),
            &["acp-relay", "agent-beta"],
        );

        assert!(result.is_ok());
    }

    #[test]
    fn the_machine_half_of_the_audience_is_not_compared() {
        let result = check_audience(&claims_for("acp-relay@some-other-name"), &["acp-relay"]);

        assert!(result.is_ok());
    }

    #[test]
    fn an_unreadable_epoch_renders_as_an_empty_string() {
        assert_eq!(iso_from_epoch(f64::MAX), "");
    }

    #[test]
    fn peer_endpoints_must_be_absolute_and_credential_free() {
        assert!(valid_endpoint(
            "https://agent.example.test:8444",
            &["http", "https"]
        ));
        assert!(!valid_endpoint(
            "agent.example.test:8444",
            &["http", "https"]
        ));
        assert!(!valid_endpoint(
            "https://user:password@agent.example.test",
            &["http", "https"]
        ));
        assert!(!valid_endpoint(
            "https://agent.example.test/path?token=secret",
            &["http", "https"]
        ));
    }
}
