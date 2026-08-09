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
async fn health() -> impl IntoResponse {
    with_cors(Json(serde_json::json!({
        "status": "ok",
        "agent": "acp-agent"
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
    with_cors(Json(serde_json::json!({
        "messages": [],
        "count": 0
    })))
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
        .route("/acp/v1/messages/send", post(send_message))
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
