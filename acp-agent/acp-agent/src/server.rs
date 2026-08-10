//! ACP Agent HTTP Server
//!
//! Axum-based HTTP server implementing ACP endpoints.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, options},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::agent::ACPAgent;

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SendRequest {
    pub envelope: acp_core::protocol::Envelope,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SendResponse {
    pub msg_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusResponse {
    pub msg_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AckRequest {
    pub ack_id: String,
    pub received: bool,
    #[serde(default)]
    pub processed: bool,
    #[serde(default)]
    pub stream_available: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ErrorRequest {
    pub error_code: String,
    pub error_message: String,
    #[serde(default = "default_true")]
    pub retryable: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StreamInitRequest {
    pub msg_id: String,
    pub corr_id: String,
    #[serde(default = "default_stream_type")]
    pub stream_type: String,
}

fn default_stream_type() -> String {
    "reply".to_string()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct StreamInitResponse {
    pub stream_id: String,
    pub ws_url: String,
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<RwLock<Option<ACPAgent>>>,
}

type SharedState = Arc<AppState>;

// ---------------------------------------------------------------------------
// CORS helper
// ---------------------------------------------------------------------------

const CORS_HEADERS: [(axum::http::HeaderName, &'static str); 4] = [
    (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
    (axum::http::header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, OPTIONS"),
    (axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS, "Authorization, Content-Type"),
    (axum::http::header::ACCESS_CONTROL_MAX_AGE, "86400"),
];

fn with_cors(body: impl IntoResponse) -> Response {
    let mut resp = body.into_response();
    let headers = resp.headers_mut();
    for (name, value) in &CORS_HEADERS {
        if let Ok(val) = value.parse() {
            headers.insert((*name).clone(), val);
        }
    }
    resp
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

// GET /health
async fn health(State(state): State<SharedState>) -> impl IntoResponse {
    let agent_guard = state.agent.read().await;
    let (agent_id, machine_id) = match agent_guard.as_ref() {
        Some(a) => (a.this.agent_id.clone(), a.this.machine_id.clone().unwrap_or_default()),
        None => ("unknown".to_string(), "unknown".to_string()),
    };
    with_cors(Json(serde_json::json!({
        "status": "ok",
        "agent": "acp-agent",
        "this_agent_id": agent_id,
        "this_machine_id": machine_id,
    })))
}

// POST /acp/v1/messages/send
async fn send_message(
    State(state): State<SharedState>,
    Json(body): Json<SendRequest>,
) -> impl IntoResponse {
    let agent_guard = state.agent.read().await;
    let agent = match agent_guard.as_ref() {
        Some(a) => a,
        None => {
            let mut resp = Json(serde_json::json!({"error": "Agent not initialized"})).into_response();
            *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            return with_cors(resp);
        }
    };

    let message = acp_core::protocol::Message {
        envelope: body.envelope,
        payload: body.payload,
    };

    // Extract target from envelope
    let target = message.envelope.recipient.agent_id.clone();

    match agent.delegate_to(&target, &message, None).await {
        Ok(_) => with_cors(Json(SendResponse {
            msg_id: message.envelope.msg_id,
            status: "accepted".to_string(),
            next_hop: Some(target),
            error: None,
        })),
        Err(e) => {
            let mut resp = Json(serde_json::json!({"error": e.to_string()})).into_response();
            *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            with_cors(resp)
        }
    }
}

// GET /acp/v1/messages/pending
async fn get_pending(State(state): State<SharedState>) -> impl IntoResponse {
    let agent_guard = state.agent.read().await;
    let agent = match agent_guard.as_ref() {
        Some(a) => a,
        None => {
            let mut resp = Json(serde_json::json!({"messages": [], "count": 0})).into_response();
            return with_cors(resp);
        }
    };
    let messages = agent.get_all_messages().await;
    with_cors(Json(serde_json::json!({
        "messages": messages,
        "count": messages.len()
    })))
}

// GET /acp/v1/debug/messages
async fn debug_messages(State(state): State<SharedState>) -> impl IntoResponse {
    let agent_guard = state.agent.read().await;
    let agent = match agent_guard.as_ref() {
        Some(a) => a,
        None => {
            let mut resp = Json(serde_json::json!({"messages": []})).into_response();
            return with_cors(resp);
        }
    };
    let messages = agent.get_all_messages().await;
    with_cors(Json(serde_json::json!({ "messages": messages })))
}

// GET /acp/v1/messages/{msg_id}/status
async fn message_status(
    State(state): State<SharedState>,
    axum::extract::Path(msg_id): axum::extract::Path<String>,
) -> Response {
    let agent_guard = state.agent.read().await;
    let agent = match agent_guard.as_ref() {
        Some(a) => a,
        None => {
            let mut resp =
                Json(serde_json::json!({"error": "Agent not initialized"})).into_response();
            *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            return with_cors(resp);
        }
    };

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
        None => {
            let mut resp = Json(serde_json::json!({"error": "NOT_FOUND"})).into_response();
            *resp.status_mut() = StatusCode::NOT_FOUND;
            with_cors(resp)
        }
    }
}

// GET /acp/v1/capabilities
//
// Unauthenticated, like /health — it reports only what this agent can speak, and
// a peer may need it before it can negotiate anything else.
async fn capabilities(State(state): State<SharedState>) -> impl IntoResponse {
    let agent_guard = state.agent.read().await;
    let (agent_id, machine_id, caps) = match agent_guard.as_ref() {
        Some(a) => (
            a.this.agent_id.clone(),
            a.this.machine_id.clone().unwrap_or_default(),
            a.this.capabilities.clone(),
        ),
        None => ("unknown".to_string(), "unknown".to_string(), Vec::new()),
    };

    with_cors(Json(serde_json::json!({
        "protocol_version": "1.0",
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

// GET /acp/v1/peers
async fn get_peers(State(state): State<SharedState>) -> impl IntoResponse {
    let agent_guard = state.agent.read().await;
    let agent = match agent_guard.as_ref() {
        Some(a) => a,
        None => {
            let mut resp = Json(serde_json::json!({"peers": []})).into_response();
            return with_cors(resp);
        }
    };
    let peers = agent.get_peers();
    let peers_json: Vec<serde_json::Value> = peers.into_iter().map(|p| {
        serde_json::json!({
            "agent_id": p.agent_id,
            "machine_id": p.machine_id,
            "http_endpoint": p.http_endpoint,
            "ws_endpoint": p.ws_endpoint,
            "capabilities": p.capabilities,
            "last_seen_at": None::<f64>,
        })
    }).collect();
    with_cors(Json(serde_json::json!({ "peers": peers_json })))
}

// POST /acp/v1/relay/forward
//
// Push delivery from the relay. The relay signs a token bound to this message
// and addressed to the envelope's recipient; we verify that, confirm the message
// is actually for us, then hand it to the normal incoming path.
async fn relay_forward(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<SendRequest>,
) -> Response {
    let this_agent_id = {
        let agent_guard = state.agent.read().await;
        match agent_guard.as_ref() {
            Some(a) => a.this.agent_id.clone(),
            None => {
                let mut resp =
                    Json(serde_json::json!({"error": "Agent not initialized"})).into_response();
                *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
                return with_cors(resp);
            }
        }
    };

    let envelope = &body.envelope;
    let msg_id = envelope.msg_id.clone();

    if envelope.recipient.agent_id != this_agent_id {
        tracing::warn!(
            "[FORWARD] {} addressed to {} but this agent is {}",
            msg_id, envelope.recipient.agent_id, this_agent_id
        );
        let mut resp = Json(serde_json::json!({
            "error": "WRONG_RECIPIENT",
            "message": format!("This agent is {}", this_agent_id),
        }))
        .into_response();
        *resp.status_mut() = StatusCode::NOT_FOUND;
        return with_cors(resp);
    }

    if let Err(reason) = verify_forward_auth(&headers, envelope) {
        tracing::warn!("[FORWARD] Rejected {}: {}", msg_id, reason);
        let mut resp =
            Json(serde_json::json!({"error": "UNAUTHORIZED", "message": reason})).into_response();
        *resp.status_mut() = StatusCode::UNAUTHORIZED;
        return with_cors(resp);
    }

    let message = acp_core::protocol::Message {
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
            tracing::error!("[FORWARD] Agent gone before processing {}", spawned_id);
        }
    });

    tracing::info!("[FORWARD] Accepted {} from relay", msg_id);
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
/// caller has already confirmed that recipient is us. Without `ACP_SHARED_SECRET`
/// the agent is in the documented trusted-network mode and cannot verify anything.
fn verify_forward_auth(
    headers: &HeaderMap,
    envelope: &acp_core::protocol::Envelope,
) -> Result<(), String> {
    let secret = match std::env::var("ACP_SHARED_SECRET") {
        Ok(s) if !s.is_empty() => s,
        _ => {
            tracing::warn!(
                "[FORWARD] ACP_SHARED_SECRET not set — accepting {} unauthenticated",
                envelope.msg_id
            );
            return Ok(());
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

// POST /acp/v1/messages/{msg_id}/ack
async fn ack_message(
    State(_state): State<SharedState>,
    axum::extract::Path(msg_id): axum::extract::Path<String>,
    Json(body): Json<AckRequest>,
) -> impl IntoResponse {
    tracing::debug!("[ACK] {} ack_id={}", msg_id, body.ack_id);
    with_cors(Json(serde_json::json!({
        "ack_id": body.ack_id,
        "recorded": true
    })))
}

// POST /acp/v1/messages/{msg_id}/error
async fn error_message(
    State(_state): State<SharedState>,
    axum::extract::Path(msg_id): axum::extract::Path<String>,
    Json(body): Json<ErrorRequest>,
) -> impl IntoResponse {
    tracing::warn!("[ERR] {} code={} msg={}", msg_id, body.error_code, body.error_message);
    with_cors(Json(serde_json::json!({
        "recorded": true
    })))
}

// POST /acp/v1/stream/init
async fn stream_init(
    State(state): State<SharedState>,
    Json(body): Json<StreamInitRequest>,
) -> impl IntoResponse {
    let agent = state.agent.read().await;
    let agent = match agent.as_ref() {
        Some(a) => a,
        None => {
            let mut resp = Json(serde_json::json!({"error": "Agent not initialized"})).into_response();
            *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            return with_cors(resp);
        }
    };

    let stream_id = acp_core::protocol::new_stream_id();
    let ws_url = format!("{}/{}", agent.this.ws_endpoint.as_deref().unwrap_or(""), stream_id);

    with_cors(Json(StreamInitResponse {
        stream_id,
        ws_url,
    }))
}

// OPTIONS /<path> for CORS preflight
async fn cors_preflight() -> impl IntoResponse {
    axum::response::AppendHeaders(CORS_HEADERS.to_vec())
}

// ---------------------------------------------------------------------------
// Server builder
// ---------------------------------------------------------------------------

pub fn build_router(agent: ACPAgent) -> Router {
    let state = Arc::new(AppState {
        agent: Arc::new(RwLock::new(Some(agent))),
    });

    Router::new()
        .route("/health", get(health))
        .route("/acp/v1/capabilities", get(capabilities))
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

pub async fn start_server(agent: ACPAgent, port: u16) -> anyhow::Result<()> {
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

    let addr = format!("0.0.0.0:{}", port);
    tracing::info!("ACP Agent HTTP server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
