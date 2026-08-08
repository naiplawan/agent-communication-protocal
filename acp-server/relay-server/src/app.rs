//! ACP Relay HTTP handlers

use crate::models::{Peer, SendRequest, SendResponse, TokenClaims};
use crate::security::{extract_agent_id, verify_token};
use crate::store::Store;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{AppendHeaders, IntoResponse, Response},
    Json, Router,
};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub shared_secret: String,
    pub this_agent_id: String,
}

type SharedState = Arc<AppState>;

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

fn verify_auth(headers: &HeaderMap, secret: &str) -> Result<TokenClaims, &'static str> {
    let auth = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or("Missing Authorization header")?;

    if !auth.starts_with("ACP-Token ") {
        return Err("Invalid authorization header format");
    }

    verify_token(&auth[10..], secret).map_err(|e| {
        warn!("Token verification failed: {}", e);
        match e {
            crate::security::TokenError::Expired => "Token expired",
            _ => "Invalid token",
        }
    })
}

// ---- Health ----

pub async fn health() -> impl IntoResponse {
    with_cors(Json(serde_json::json!({ "status": "ok", "agent": "acp-relay" })))
}

// ---- Register ----

pub async fn register(
    State(state): State<SharedState>,
    Json(peer): Json<Peer>,
) -> impl IntoResponse {
    info!("[REG] Registering peer: {}", peer.agent_id);
    if let Err(e) = state.store.register_peer(&peer) {
        error!("[REG] Failed to register peer: {:?}", e);
        return error_resp("DB_ERROR", StatusCode::INTERNAL_SERVER_ERROR);
    }
    let peers = state.store.get_peers(false).unwrap_or_default();
    with_cors(Json(serde_json::json!({
        "status": "registered",
        "agent_id": peer.agent_id,
        "peers": peers,
        "this_agent": { "agent_id": state.this_agent_id }
    })))
}

// ---- Peers ----

pub async fn get_peers(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(msg) = verify_auth(&headers, &state.shared_secret) {
        return auth_error(msg);
    }
    let peers = state.store.get_peers(false).unwrap_or_default();
    with_cors(Json(serde_json::json!({ "peers": peers })))
}

// ---- Send message ----

pub async fn send_message(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<SendRequest>,
) -> impl IntoResponse {
    if let Err(msg) = verify_auth(&headers, &state.shared_secret) {
        return auth_error(msg);
    }

    let envelope = &body.envelope;
    let msg_id = &envelope.msg_id;
    let recipient_agent = &envelope.recipient.agent_id;
    let recipient_machine = envelope.recipient.machine_id.as_deref();

    info!("[SEND] msg_id={} -> {}", &msg_id[..8.min(msg_id.len())], recipient_agent);

    // Local delivery
    if recipient_agent == &state.this_agent_id {
        if let Err(e) = state.store.put(msg_id, envelope, &body.payload) {
            error!("[SEND] Local put failed: {:?}", e);
        }
        state.store.update_status(msg_id, "accepted").ok();
        return with_cors(send_ok(msg_id, "accepted", None));
    }

    // Try dynamic peer lookup (active peers only)
    let peers = state.store.get_peers(false).unwrap_or_default();
    let recipient_config = peers.iter().find(|p| p.agent_id == *recipient_agent);

    if recipient_config.is_none() {
        // Broker mode
        if let Err(e) = state.store.put(msg_id, envelope, &body.payload) {
            error!("[SEND] Broker put failed: {:?}", e);
            return error_resp("DB_ERROR", StatusCode::INTERNAL_SERVER_ERROR);
        }
        info!("[SEND] Brokered msg {} for {}", msg_id, recipient_agent);
        return with_cors(send_ok(msg_id, "brokered", None));
    }

    let peer = recipient_config.unwrap();
    let forward_url = format!("{}/acp/v1/relay/forward", peer.http_endpoint);

    let mut new_envelope = envelope.clone();
    new_envelope.sender.agent_id = state.this_agent_id.clone();
    new_envelope.sender.machine_id = Some("relay".to_string());
    if let Some(hops) = &mut new_envelope.hops {
        hops.count += 1;
        hops.trace.push(format!("{}@relay", state.this_agent_id));
    } else {
        new_envelope.hops = Some(crate::models::Hops {
            count: 1, max: 10,
            trace: vec![format!("{}@relay", state.this_agent_id)],
        });
    }

    let forward_token = create_token(
        &state.this_agent_id, "relay",
        recipient_agent, recipient_machine.unwrap_or(""),
        msg_id, &state.shared_secret,
    );

    let client = reqwest::Client::new();
    match client
        .post(&forward_url)
        .json(&serde_json::json!({ "envelope": new_envelope, "payload": body.payload }))
        .header("Authorization", format!("ACP-Token {}", forward_token))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            state.store.put(msg_id, envelope, &body.payload).ok();
            info!("[SEND] Forwarded {} to {}", msg_id, peer.http_endpoint);
            with_cors(send_ok(msg_id, "forwarded", Some(&peer.http_endpoint)))
        }
        Ok(resp) => {
            warn!("[SEND] Forward failed {}: {}", resp.status(), resp.text().await.unwrap_or_default());
            state.store.put(msg_id, envelope, &body.payload).ok();
            with_cors(send_ok(msg_id, "brokered", None))
        }
        Err(e) => {
            let err_str = format!("{}", e);
            error!("[SEND] Forward error: {}", err_str);
            if err_str.contains("Connection refused") || err_str.contains("connection refused")
                || err_str.contains("connection reset") || err_str.contains("error sending request")
                || err_str.contains("Connection timed out") || err_str.contains("connect)")
            {
                state.store.remove_peer(recipient_agent).ok();
                warn!("[SEND] Removed stale peer {} - re-brokering", recipient_agent);
                state.store.put(msg_id, envelope, &body.payload).ok();
                return with_cors(send_ok(msg_id, "brokered", None));
            }
            error_resp("FORWARD_ERROR", StatusCode::BAD_GATEWAY)
        }
    }
}

// ---- Poll pending ----

pub async fn get_pending(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let claims = match verify_auth(&headers, &state.shared_secret) {
        Ok(c) => c,
        Err(msg) => return auth_error(msg),
    };
    let agent_id = agent_id_from_iss(&claims.iss);
    let messages = state.store.get_all_pending(&agent_id).unwrap_or_default();
    info!("[PENDING] {} messages for {}", messages.len(), agent_id);
    with_cors(Json(serde_json::json!({ "messages": messages, "count": messages.len() })))
}

// ---- Debug messages ----

pub async fn debug_messages(State(state): State<SharedState>) -> impl IntoResponse {
    let messages = state.store.get_all_messages().unwrap_or_default();
    with_cors(Json(serde_json::json!({ "messages": messages })))
}

// ---- CORS preflight ----

pub async fn cors_preflight() -> impl IntoResponse {
    AppendHeaders(CORS_HEADERS.to_vec())
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
    let mut resp = Json(serde_json::json!({ "error": "UNAUTHORIZED", "message": msg })).into_response();
    *resp.status_mut() = StatusCode::UNAUTHORIZED;
    with_cors(resp)
}

fn agent_id_from_iss(iss: &str) -> String {
    extract_agent_id(iss)
}

// ---- Token creation ----

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0)).collect()
}

pub fn create_token(
    issuer_agent_id: &str, issuer_machine_id: &str,
    audience_agent_id: &str, audience_machine_id: &str,
    msg_id: &str, secret: &str,
) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;

    let header = b"{\"alg\":\"HS256\",\"typ\":\"ACP\"}";
    let header_b64 = URL_SAFE_NO_PAD.encode(header);

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);

    let payload = serde_json::json!({
        "iss": format!("{}@{}", issuer_agent_id, issuer_machine_id),
        "aud": format!("{}@{}", audience_agent_id, audience_machine_id),
        "exp": chrono::DateTime::from_timestamp(now_secs as i64 + 3600, 0).unwrap().to_rfc3339(),
        "iat": chrono::DateTime::from_timestamp(now_secs as i64, 0).unwrap().to_rfc3339(),
        "msg_id": msg_id,
        "nonce": uuid::Uuid::new_v4().to_string(),
    });
    let payload_bytes = serde_json::to_vec(&payload).unwrap();
    let payload_b64 = URL_SAFE_NO_PAD.encode(&payload_bytes);

    let signing_input = format!("{}.{}", header_b64, payload_b64);
    let key = hex_to_bytes(secret);
    let mut mac = HmacSha256::new_from_slice(&key).unwrap();
    mac.update(signing_input.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig.as_slice());

    format!("{}.{}.{}", header_b64, payload_b64, sig_b64)
}

// ---- Build router ----

pub fn build_router(state: AppState) -> Router {
    let s = Arc::new(state);
    Router::new()
        .route("/health", axum::routing::get(health))
        .route("/acp/v1/agents/register", axum::routing::post(register))
        .route("/acp/v1/peers", axum::routing::get(get_peers))
        .route("/acp/v1/messages/send", axum::routing::post(send_message))
        .route("/acp/v1/messages/pending", axum::routing::get(get_pending))
        .route("/acp/v1/debug/messages", axum::routing::get(debug_messages))
        .route("/<path:_>", axum::routing::options(cors_preflight))
        .with_state(s)
}
