//! ACP Agent HTTP Server
//!
//! Axum-based HTTP server implementing the ACP endpoints an agent must expose.

use std::sync::Arc;

use acp_core::protocol::{
    negotiate_protocol_version, new_stream_id, Envelope, Message, PROTOCOL_VERSION,
    SUPPORTED_PROTOCOL_VERSIONS,
};
use acp_core::security::{verify_token, TokenPayload};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{AppendHeaders, IntoResponse, Response};
use axum::routing::{get, options, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::agent::ACPAgent;

/// Environment variable holding the shared secret used to verify relay pushes.
const SHARED_SECRET_ENV: &str = "ACP_SHARED_SECRET";

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

/// Body of `POST /acp/v1/messages/send` and `/acp/v1/relay/forward`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SendRequest {
    /// Routing metadata.
    pub envelope: Envelope,
    /// Application data.
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

/// Response to an accepted message.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SendResponse {
    /// Message the response is about.
    pub msg_id: String,
    /// What this agent did with it.
    pub status: String,
    /// Agent it was forwarded to, when it was forwarded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,
    /// Why it was refused, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response to `GET /acp/v1/messages/{msg_id}/status`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusResponse {
    /// Message the response is about.
    pub msg_id: String,
    /// Where the message stands.
    pub status: String,
    /// When it was delivered, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<String>,
}

/// Body of `POST /acp/v1/messages/{msg_id}/ack`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AckRequest {
    /// Identifier the sender assigned to this acknowledgement.
    pub ack_id: String,
    /// The sender has the message.
    pub received: bool,
    /// The sender has finished handling it.
    #[serde(default)]
    pub processed: bool,
    /// A streamed reply is available for it.
    #[serde(default)]
    pub stream_available: bool,
}

/// Body of `POST /acp/v1/messages/{msg_id}/error`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ErrorRequest {
    /// Machine-readable failure code.
    pub error_code: String,
    /// Human-readable failure description.
    pub error_message: String,
    /// Whether resending could succeed.
    #[serde(default = "default_true")]
    pub retryable: bool,
}

fn default_true() -> bool {
    true
}

/// Body of `POST /acp/v1/stream/init`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StreamInitRequest {
    /// Message the stream will answer.
    pub msg_id: String,
    /// Correlation ID of the exchange.
    pub corr_id: String,
    /// Session this stream belongs to.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Run this stream belongs to.
    #[serde(default)]
    pub run_id: Option<String>,
    /// What the stream carries, e.g. `"reply"`.
    #[serde(default = "default_stream_type")]
    pub stream_type: String,
}

fn default_stream_type() -> String {
    "reply".to_string()
}

/// Response to `POST /acp/v1/stream/init`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct StreamInitResponse {
    /// Identifier for the opened stream.
    pub stream_id: String,
    /// WebSocket URL to send frames to.
    pub ws_url: String,
}

/// Body of `POST /acp/v1/initialize`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
struct InitializeRequest {
    /// Protocol versions the caller can speak, newest first.
    #[serde(default)]
    protocol_versions: Vec<String>,
    /// Features the caller would like to use.
    #[serde(default)]
    capabilities: Vec<String>,
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

/// The agent this server fronts.
///
/// Held as an `Option` because a request may arrive before the agent is
/// installed, or after it has been torn down.
#[derive(Clone)]
pub struct AppState {
    /// The agent, when one is installed.
    pub agent: Arc<RwLock<Option<ACPAgent>>>,
}

type SharedState = Arc<AppState>;

// ---------------------------------------------------------------------------
// CORS helper
// ---------------------------------------------------------------------------

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

fn error_resp(error: &str, status: StatusCode) -> Response {
    let mut resp = Json(serde_json::json!({ "error": error })).into_response();
    *resp.status_mut() = status;
    with_cors(resp)
}

fn agent_unavailable() -> Response {
    error_resp("Agent not initialized", StatusCode::SERVICE_UNAVAILABLE)
}

/// Verify a token addressed to this agent and optionally bound to one message.
fn authenticate(
    headers: &HeaderMap,
    agent: &ACPAgent,
    required_msg_id: Option<&str>,
) -> Result<TokenPayload, Box<Response>> {
    let secret = match std::env::var(SHARED_SECRET_ENV) {
        Ok(secret) if !secret.is_empty() => secret,
        _ => {
            return Err(Box::new(error_resp(
                "Authentication is required",
                StatusCode::UNAUTHORIZED,
            )))
        }
    };
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("ACP-Token "))
        .ok_or_else(|| {
            Box::new(error_resp(
                "Invalid Authorization header",
                StatusCode::UNAUTHORIZED,
            ))
        })?;

    verify_token(
        auth,
        &secret,
        &agent.this.agent_id,
        agent.this.machine_id.as_deref().unwrap_or_default(),
        required_msg_id,
    )
    .map_err(|error| Box::new(error_resp(&error.to_string(), StatusCode::UNAUTHORIZED)))
}

fn authenticate_observability(headers: &HeaderMap, agent: &ACPAgent) -> Option<Response> {
    if std::env::var("ACP_PUBLIC_DEBUG")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        tracing::warn!("ACP_PUBLIC_DEBUG is enabled; exposing authenticated diagnostics publicly");
        return None;
    }
    authenticate(headers, agent, None)
        .err()
        .map(|response| *response)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /health` — liveness, unauthenticated.
async fn health(State(state): State<SharedState>) -> Response {
    let agent_guard = state.agent.read().await;
    let (agent_id, machine_id) = agent_guard.as_ref().map_or_else(
        || ("unknown".to_string(), "unknown".to_string()),
        |a| {
            (
                a.this.agent_id.clone(),
                a.this.machine_id.clone().unwrap_or_default(),
            )
        },
    );
    with_cors(Json(serde_json::json!({
        "status": "ok",
        "agent": "acp-agent",
        "this_agent_id": agent_id,
        "this_machine_id": machine_id,
    })))
}

/// `POST /acp/v1/messages/send` — forward a message to its envelope recipient.
async fn send_message(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<SendRequest>,
) -> Response {
    let agent_guard = state.agent.read().await;
    let Some(agent) = agent_guard.as_ref() else {
        return agent_unavailable();
    };
    let claims = match authenticate(&headers, agent, Some(&body.envelope.msg_id)) {
        Ok(claims) => claims,
        Err(response) => return *response,
    };
    let expected_sender = format!(
        "{}@{}",
        body.envelope.sender.agent_id,
        body.envelope
            .sender
            .machine_id
            .as_deref()
            .unwrap_or_default()
    );
    if claims.iss != expected_sender {
        return error_resp(
            "Token issuer does not match envelope sender",
            StatusCode::FORBIDDEN,
        );
    }

    let message = Message {
        envelope: body.envelope,
        payload: body.payload,
    };
    let target = message.envelope.recipient.agent_id.clone();

    match agent.delegate_to(&target, &message, None).await {
        Ok(_) => with_cors(Json(SendResponse {
            msg_id: message.envelope.msg_id,
            status: "accepted".to_string(),
            next_hop: Some(target),
            error: None,
        })),
        Err(e) => error_resp(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// `GET /acp/v1/messages/pending` — everything this agent knows about.
async fn get_pending(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    let agent_guard = state.agent.read().await;
    if let Some(agent) = agent_guard.as_ref() {
        if let Err(response) = authenticate(&headers, agent, Some("poll_pending")) {
            return *response;
        }
    }
    let messages = collect_messages(&state).await;
    with_cors(Json(serde_json::json!({
        "count": messages.len(),
        "messages": messages,
    })))
}

/// `GET /acp/v1/debug/messages` — the same list, without the count.
async fn debug_messages(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    let agent_guard = state.agent.read().await;
    if let Some(agent) = agent_guard.as_ref() {
        if let Some(response) = authenticate_observability(&headers, agent) {
            return response;
        }
    }
    let messages = collect_messages(&state).await;
    with_cors(Json(serde_json::json!({ "messages": messages })))
}

async fn collect_messages(state: &SharedState) -> Vec<serde_json::Value> {
    let agent_guard = state.agent.read().await;
    match agent_guard.as_ref() {
        Some(agent) => agent.get_all_messages().await,
        None => Vec::new(),
    }
}

/// `GET /acp/v1/messages/{msg_id}/status` — where one message stands.
async fn message_status(
    State(state): State<SharedState>,
    Path(msg_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let agent_guard = state.agent.read().await;
    let Some(agent) = agent_guard.as_ref() else {
        return agent_unavailable();
    };
    if let Err(response) = authenticate(&headers, agent, Some(&msg_id)) {
        return *response;
    }

    let status = agent
        .get_all_messages()
        .await
        .into_iter()
        .find(|m| m.get("msg_id").and_then(|v| v.as_str()) == Some(msg_id.as_str()))
        .and_then(|m| m.get("status").and_then(|v| v.as_str()).map(String::from));

    match status {
        Some(status) => with_cors(Json(StatusResponse {
            msg_id,
            status,
            delivered_at: None,
        })),
        None => error_resp("NOT_FOUND", StatusCode::NOT_FOUND),
    }
}

/// `GET /acp/v1/capabilities` — what this agent can speak.
///
/// Unauthenticated, like `/health`: it reports only this agent's own protocol
/// support, and a peer may need it before it can negotiate anything else.
async fn capabilities(State(state): State<SharedState>) -> Response {
    let agent_guard = state.agent.read().await;
    let (agent_id, machine_id, caps) = agent_guard.as_ref().map_or_else(
        || ("unknown".to_string(), "unknown".to_string(), Vec::new()),
        |a| {
            (
                a.this.agent_id.clone(),
                a.this.machine_id.clone().unwrap_or_default(),
                a.this.capabilities.clone(),
            )
        },
    );

    with_cors(Json(serde_json::json!({
        "protocol_version": PROTOCOL_VERSION,
        "agent_id": agent_id,
        "machine_id": machine_id,
        "role": "agent",
        "capabilities": caps,
        "intents": [
            "delegate", "reply", "ack", "error",
            "stream_start", "stream_chunk", "stream_end",
        ],
        "content_types": ["application/json"],
        "auth": ["signed-token"],
    })))
}

/// `POST /acp/v1/initialize` — authenticate and negotiate protocol features.
async fn initialize(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<InitializeRequest>,
) -> Response {
    let agent_guard = state.agent.read().await;
    let Some(agent) = agent_guard.as_ref() else {
        return agent_unavailable();
    };
    if let Err(response) = authenticate(&headers, agent, Some("initialize")) {
        return *response;
    }

    let Some(protocol_version) = negotiate_protocol_version(&body.protocol_versions) else {
        return error_resp("UNSUPPORTED_PROTOCOL_VERSION", StatusCode::BAD_REQUEST);
    };

    let features = ["session-context", "run-context", "streaming"];
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
        "role": "agent",
        "agent_id": agent.this.agent_id,
        "machine_id": agent.this.machine_id.clone().unwrap_or_default(),
        "capabilities": agent.this.capabilities,
        "accepted_capabilities": accepted_capabilities,
        "features": features,
        "intents": [
            "delegate", "reply", "ack", "error",
            "stream_start", "stream_chunk", "stream_end",
        ],
        "content_types": ["application/json"],
        "auth": ["signed-token"],
    })))
}

/// `GET /acp/v1/peers` — the peers this agent is configured to reach.
async fn get_peers(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    let agent_guard = state.agent.read().await;
    let Some(agent) = agent_guard.as_ref() else {
        return with_cors(Json(serde_json::json!({ "peers": [] })));
    };
    if let Some(response) = authenticate_observability(&headers, agent) {
        return response;
    }

    let peers: Vec<serde_json::Value> = agent
        .get_peers()
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "agent_id": p.agent_id,
                "machine_id": p.machine_id,
                "http_endpoint": p.http_endpoint,
                "ws_endpoint": p.ws_endpoint,
                "capabilities": p.capabilities,
                "last_seen_at": None::<f64>,
            })
        })
        .collect();
    with_cors(Json(serde_json::json!({ "peers": peers })))
}

/// `POST /acp/v1/relay/forward` — push delivery from the relay.
///
/// The relay signs a token bound to this message and addressed to the envelope's
/// recipient; that is verified, the message is confirmed to be for this agent,
/// and then it joins the normal incoming path.
async fn relay_forward(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<SendRequest>,
) -> Response {
    let this_agent_id = {
        let agent_guard = state.agent.read().await;
        match agent_guard.as_ref() {
            Some(a) => a.this.agent_id.clone(),
            None => return agent_unavailable(),
        }
    };

    let envelope = &body.envelope;
    let msg_id = envelope.msg_id.clone();

    if envelope.recipient.agent_id != this_agent_id {
        tracing::warn!(
            "[FORWARD] {msg_id} addressed to {} but this agent is {this_agent_id}",
            envelope.recipient.agent_id,
        );
        let mut resp = Json(serde_json::json!({
            "error": "WRONG_RECIPIENT",
            "message": format!("This agent is {this_agent_id}"),
        }))
        .into_response();
        *resp.status_mut() = StatusCode::NOT_FOUND;
        return with_cors(resp);
    }

    if let Err(reason) = verify_forward_auth(&headers, envelope) {
        tracing::warn!("[FORWARD] Rejected {msg_id}: {reason}");
        let mut resp =
            Json(serde_json::json!({"error": "UNAUTHORIZED", "message": reason})).into_response();
        *resp.status_mut() = StatusCode::UNAUTHORIZED;
        return with_cors(resp);
    }

    let message = Message {
        envelope: body.envelope,
        payload: body.payload,
    };

    // Processing can invoke a long-running handler and auto-reply, so acknowledge
    // the hop now and let it run — the relay's forward call times out in 10s.
    let agent_state = state.agent.clone();
    let spawned_id = msg_id.clone();
    tokio::spawn(async move {
        let agent_guard = agent_state.read().await;
        if let Some(agent) = agent_guard.as_ref() {
            agent.handle_incoming(message).await;
        } else {
            tracing::error!("[FORWARD] Agent gone before processing {spawned_id}");
        }
    });

    tracing::info!("[FORWARD] Accepted {msg_id} from relay");
    let mut resp = Json(SendResponse {
        msg_id,
        status: "accepted".to_string(),
        next_hop: None,
        error: None,
    })
    .into_response();
    *resp.status_mut() = StatusCode::ACCEPTED;
    with_cors(resp)
}

/// Verify a relay-signed forward token.
///
/// The audience is checked against the envelope's own recipient rather than this
/// agent's configured identity: the relay derives it from the same field, and the
/// caller has already confirmed that recipient is us. Unauthenticated forwarding
/// is available only through an explicit development-only opt-in.
fn verify_forward_auth(headers: &HeaderMap, envelope: &Envelope) -> Result<(), String> {
    let secret = match std::env::var(SHARED_SECRET_ENV) {
        Ok(s) if !s.is_empty() => s,
        _ if std::env::var("ACP_ALLOW_UNAUTHENTICATED_FORWARD")
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(false) =>
        {
            tracing::warn!(
                "[FORWARD] ACP_ALLOW_UNAUTHENTICATED_FORWARD is enabled — accepting {} unauthenticated",
                envelope.msg_id
            );
            return Ok(());
        }
        _ => {
            return Err(format!(
                "{SHARED_SECRET_ENV} is required for forwarded messages"
            ))
        }
    };

    let header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| "Missing Authorization header".to_string())?;

    let token = header
        .strip_prefix("ACP-Token ")
        .ok_or_else(|| "Expected 'ACP-Token <token>' authorization".to_string())?;

    acp_core::security::verify_token(
        token,
        &secret,
        &envelope.recipient.agent_id,
        envelope.recipient.machine_id.as_deref().unwrap_or(""),
        Some(&envelope.msg_id),
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// `POST /acp/v1/messages/{msg_id}/ack` — record a peer's acknowledgement.
async fn ack_message(
    State(state): State<SharedState>,
    Path(msg_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AckRequest>,
) -> Response {
    let agent_guard = state.agent.read().await;
    let Some(agent) = agent_guard.as_ref() else {
        return agent_unavailable();
    };
    if let Err(response) = authenticate(&headers, agent, Some(&msg_id)) {
        return *response;
    }
    tracing::debug!(
        "[ACK] {msg_id} ack_id={} received={} processed={} stream_available={}",
        body.ack_id,
        body.received,
        body.processed,
        body.stream_available,
    );
    with_cors(Json(serde_json::json!({
        "ack_id": body.ack_id,
        "recorded": true,
    })))
}

/// `POST /acp/v1/messages/{msg_id}/error` — record a peer's failure report.
async fn error_message(
    State(state): State<SharedState>,
    Path(msg_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ErrorRequest>,
) -> Response {
    let agent_guard = state.agent.read().await;
    let Some(agent) = agent_guard.as_ref() else {
        return agent_unavailable();
    };
    if let Err(response) = authenticate(&headers, agent, Some(&msg_id)) {
        return *response;
    }
    tracing::warn!(
        "[ERR] {msg_id} code={} retryable={} msg={}",
        body.error_code,
        body.retryable,
        body.error_message,
    );
    with_cors(Json(serde_json::json!({ "recorded": true })))
}

/// `POST /acp/v1/stream/init` — allocate a stream ID and hand back its URL.
async fn stream_init(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<StreamInitRequest>,
) -> Response {
    let agent_guard = state.agent.read().await;
    let Some(agent) = agent_guard.as_ref() else {
        return agent_unavailable();
    };
    if let Err(response) = authenticate(&headers, agent, Some(&body.msg_id)) {
        return *response;
    }

    let stream_id = new_stream_id();
    tracing::debug!(
        "[STREAM] {stream_id} type={} for msg={} corr={} session={:?} run={:?}",
        body.stream_type,
        body.msg_id,
        body.corr_id,
        body.session_id,
        body.run_id,
    );

    let ws_url = format!(
        "{}/{stream_id}",
        agent.this.ws_endpoint.as_deref().unwrap_or_default()
    );
    with_cors(Json(StreamInitResponse { stream_id, ws_url }))
}

/// `OPTIONS /<path>` — CORS preflight.
async fn cors_preflight() -> impl IntoResponse {
    AppendHeaders(CORS_HEADERS.to_vec())
}

// ---------------------------------------------------------------------------
// Server builder
// ---------------------------------------------------------------------------

/// Build the ACP router around `agent`.
pub fn build_router(agent: ACPAgent) -> Router {
    let state = Arc::new(AppState {
        agent: Arc::new(RwLock::new(Some(agent))),
    });

    Router::new()
        .route("/health", get(health))
        .route("/acp/v1/capabilities", get(capabilities))
        .route("/acp/v1/initialize", post(initialize))
        .route("/acp/v1/messages/{msg_id}/status", get(message_status))
        .route("/acp/v1/debug/messages", get(debug_messages))
        .route("/acp/v1/peers", get(get_peers))
        .route("/acp/v1/messages/send", post(send_message))
        .route("/acp/v1/relay/forward", post(relay_forward))
        .route("/acp/v1/messages/pending", get(get_pending))
        .route("/acp/v1/messages/{msg_id}/ack", post(ack_message))
        .route("/acp/v1/messages/{msg_id}/error", post(error_message))
        .route("/acp/v1/stream/init", post(stream_init))
        .route("/{*path}", options(cors_preflight))
        .with_state(state)
}

/// Serve the ACP router on `0.0.0.0:{port}` until the process ends.
///
/// # Errors
/// Returns the [`std::io::Error`] from binding the port or from the accept loop.
pub async fn start_server(agent: ACPAgent, port: u16) -> Result<(), std::io::Error> {
    use tower_http::cors::{Any, CorsLayer};
    use tower_http::trace::TraceLayer;

    let app = build_router(agent)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http());

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("ACP Agent HTTP server starting on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await
}
