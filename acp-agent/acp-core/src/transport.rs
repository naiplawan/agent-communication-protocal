//! ACP Transport — HTTP client with signed-token auth + retry, WebSocket client

use crate::config::{ACPConfig, Peer};
use crate::protocol::{Message, new_ack_id};
use crate::security::{create_token, PeerAuth};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum TransportError {
    #[error("HTTP {status}: {message}")]
    HttpError { status: u16, message: String },

    #[error("Cannot reach {0}: {1}")]
    Unreachable(String, String),

    #[error("Auth failed for {0}: {1}")]
    SecurityError(String, String),

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Max retry attempts exceeded")]
    MaxRetriesExceeded,

    #[error("Transport error: {0}")]
    Transport(String),
}

#[derive(Error, Debug)]
pub enum AckTimeoutError {
    #[error("Failed to deliver {msg_id} to {peer} after {attempts} attempts")]
    DeliveryFailed { msg_id: String, peer: String, attempts: u32 },
}

// ---------------------------------------------------------------------------
// HTTP Client
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ACPHttpClient {
    config: Arc<ACPConfig>,
    this_agent_id: String,
    this_machine_id: String,
    http_client: reqwest::Client,
}

impl ACPHttpClient {
    pub fn new(config: ACPConfig, this_agent_id: String, this_machine_id: String) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            config: Arc::new(config),
            this_agent_id,
            this_machine_id,
            http_client,
        }
    }

    fn get_peer_auth(&self, peer: &Peer) -> PeerAuth {
        peer.auth.clone().unwrap_or_else(|| {
            let mut auth = PeerAuth::new_signed_token();
            auth
        })
    }

    fn build_auth_header(&self, peer: &Peer, msg_id: &str) -> Option<String> {
        let auth = self.get_peer_auth(peer);
        if auth.auth_type == "signed-token" {
            let secret = auth
                .get_secret()
                .or_else(|| std::env::var("ACP_SHARED_SECRET").ok());

            let secret = match secret {
                Some(s) => s,
                None => {
                    tracing::warn!(
                        "No secret found for peer {}. Set ACP_SHARED_SECRET or configure peer auth.",
                        peer.agent_id
                    );
                    return None;
                }
            };

            let token = create_token(
                &self.this_agent_id,
                &self.this_machine_id,
                &peer.agent_id,
                &peer.machine_id,
                msg_id,
                &secret,
                self.config.security.token_ttl_seconds as i64,
            );
            Some(format!("ACP-Token {}", token))
        } else {
            None // mTLS handled at TLS layer
        }
    }

    async fn do_request(
        &self,
        method: reqwest::Method,
        url: &str,
        peer: Option<&Peer>,
        msg_id: Option<&str>,
        json_body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, TransportError> {
        let mut req = self.http_client.request(method, url);

        req = req.header("Content-Type", "application/json");
        req = req.header("Accept", "application/json");
        req = req.header("User-Agent", "acp-rust/0.1");

        if let (Some(peer), Some(msg_id)) = (peer, msg_id) {
            if let Some(auth_header) = self.build_auth_header(peer, msg_id) {
                req = req.header("Authorization", auth_header);
            }
        }

        if let Some(body) = json_body {
            req = req.json(&body);
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                TransportError::Timeout(30000)
            } else if e.is_connect() {
                TransportError::Unreachable(url.to_string(), e.to_string())
            } else {
                TransportError::Unreachable(url.to_string(), e.to_string())
            }
        })?;

        let status = resp.status().as_u16();

        if status == 401 || status == 403 {
            let body = resp.text().await.unwrap_or_default();
            return Err(TransportError::SecurityError(
                peer.map(|p| p.agent_id.clone()).unwrap_or_default(),
                body,
            ));
        }

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(TransportError::HttpError {
                status,
                message: body,
            });
        }

        let body = resp.text().await.unwrap_or_default();
        if body.is_empty() {
            Ok(serde_json::Value::Null)
        } else {
            serde_json::from_str(&body).map_err(|e| {
                TransportError::HttpError {
                    status: 200,
                    message: format!("JSON parse error: {}", e),
                }
            })
        }
    }

    // ---- Message operations ----

    /// POST /acp/v1/messages/send
    pub async fn send_message(
        &self,
        peer: &Peer,
        message: &Message,
    ) -> Result<serde_json::Value, TransportError> {
        let url = format!("{}/messages/send", peer.http_endpoint);
        self.do_request(
            reqwest::Method::POST,
            &url,
            Some(peer),
            Some(&message.envelope.msg_id),
            Some(serde_json::to_value(message).unwrap()),
        )
        .await
    }

    /// GET /acp/v1/messages/{msg_id}/status
    pub async fn get_message_status(
        &self,
        peer: &Peer,
        msg_id: &str,
    ) -> Result<serde_json::Value, TransportError> {
        let url = format!("{}/messages/{}/status", peer.http_endpoint, msg_id);
        self.do_request(reqwest::Method::GET, &url, Some(peer), Some(msg_id), None)
            .await
    }

    /// POST /acp/v1/messages/{msg_id}/ack
    pub async fn ack_message(
        &self,
        peer: &Peer,
        msg_id: &str,
        ack_type: &str,
        received: bool,
        processed: bool,
        stream_available: bool,
    ) -> Result<serde_json::Value, TransportError> {
        let url = format!("{}/messages/{}/ack", peer.http_endpoint, msg_id);
        let body = serde_json::json!({
            "ack_id": new_ack_id(),
            "received": received,
            "processed": processed,
            "stream_available": stream_available,
        });
        self.do_request(reqwest::Method::POST, &url, Some(peer), Some(msg_id), Some(body))
            .await
    }

    /// POST /acp/v1/messages/{msg_id}/error
    pub async fn report_error(
        &self,
        peer: &Peer,
        msg_id: &str,
        error_code: &str,
        error_message: &str,
        retryable: bool,
    ) -> Result<serde_json::Value, TransportError> {
        let url = format!("{}/messages/{}/error", peer.http_endpoint, msg_id);
        let body = serde_json::json!({
            "error_code": error_code,
            "error_message": error_message,
            "retryable": retryable,
        });
        self.do_request(reqwest::Method::POST, &url, Some(peer), Some(msg_id), Some(body))
            .await
    }

    /// POST /acp/v1/stream/init
    pub async fn init_stream(
        &self,
        peer: &Peer,
        msg_id: &str,
        corr_id: &str,
        stream_type: &str,
    ) -> Result<serde_json::Value, TransportError> {
        let url = format!("{}/stream/init", peer.http_endpoint);
        let body = serde_json::json!({
            "msg_id": msg_id,
            "corr_id": corr_id,
            "stream_type": stream_type,
        });
        self.do_request(
            reqwest::Method::POST,
            &url,
            Some(peer),
            Some(msg_id),
            Some(body),
        )
        .await
    }

    // ---- Retry loop ----

    /// Send a message with exponential-backoff retry until ack received
    #[allow(clippy::too_many_arguments)]
    pub async fn send_with_retry(
        &self,
        peer: &Peer,
        message: &Message,
    ) -> Result<serde_json::Value, TransportError> {
        let cfg = &self.config.retry;
        let timeout_ms = self.config.timeouts.hop_ack_ms;
        let timeout_s = Duration::from_millis(timeout_ms as u64);

        let mut backoff_ms = cfg.initial_backoff_ms as u64;
        let mut attempts = 0u32;

        loop {
            attempts += 1;
            match self.send_message(peer, message).await {
                Ok(resp) => return Ok(resp),
                Err(TransportError::Unreachable(_, _))
                | Err(TransportError::Timeout(_))
                | Err(TransportError::HttpError { .. }) => {
                    if attempts >= cfg.max_attempts as u32 {
                        return Err(TransportError::Unreachable(
                            peer.agent_id.clone(),
                            format!("Max retries ({}) exceeded", cfg.max_attempts),
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms =
                        (backoff_ms as f64 * cfg.backoff_multiplier as f64) as u64;
                    backoff_ms = backoff_ms.min(cfg.max_backoff_ms as u64);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket Client
// ---------------------------------------------------------------------------

use futures_util::StreamExt;

#[derive(Debug, Clone)]
pub struct StreamFrame {
    pub frame: serde_json::Value,
    pub data: serde_json::Value,
}

pub struct ACPWebSocketClient {
    url: String,
    inner: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
}

impl ACPWebSocketClient {
    pub async fn connect(url: &str, token: Option<String>) -> Result<Self, TransportError> {
        let request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(url)
            .header("User-Agent", "acp-rust/0.1")
            .header("Authorization", token.map(|t| format!("ACP-Token {}", t)).unwrap_or_default())
            .body(())
            .map_err(|e| TransportError::Transport(e.to_string()))?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| TransportError::Unreachable(url.to_string(), e.to_string()))?;

        Ok(Self {
            url: url.to_string(),
            inner: ws_stream,
        })
    }

    pub async fn send_frame(&mut self, frame: serde_json::Value) -> Result<(), TransportError> {
        use futures_util::SinkExt;

        let text = serde_json::to_string(&frame).map_err(|e| {
            TransportError::HttpError {
                status: 0,
                message: format!("JSON serialize error: {}", e),
            }
        })?;

        self.inner
            .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
            .await
            .map_err(|e| TransportError::Unreachable(self.url.clone(), e.to_string()))?;

        Ok(())
    }

    pub async fn send_chunk(
        &mut self,
        stream_id: &str,
        msg_id: &str,
        corr_id: &str,
        seq: u32,
        total: Option<u32>,
        final_: bool,
        data: serde_json::Value,
    ) -> Result<(), TransportError> {
        let frame = crate::protocol::build_ws_frame(stream_id, msg_id, corr_id, seq, total, final_, data);
        self.send_frame(frame).await
    }

    pub async fn recv_frame(&mut self) -> Result<StreamFrame, TransportError> {
        let msg = match self.inner.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => return Err(TransportError::Transport(e.to_string())),
            None => return Err(TransportError::Unreachable(self.url.clone(), "WebSocket closed".to_string())),
        };

        let text = msg.to_text().map_err(|e| {
            TransportError::HttpError {
                status: 0,
                message: format!("WebSocket text error: {}", e),
            }
        })?;

        let raw: serde_json::Value = serde_json::from_str(text).map_err(|e| {
            TransportError::HttpError {
                status: 0,
                message: format!("JSON parse error: {}", e),
            }
        })?;

        let frame = raw.get("frame").cloned().unwrap_or(serde_json::Value::Null);
        let data = raw.get("data").cloned().unwrap_or(serde_json::Value::Null);

        Ok(StreamFrame { frame, data })
    }

    pub async fn close(mut self) -> Result<(), TransportError> {
        self.inner.close(None).await.map_err(|e| {
            TransportError::Unreachable(self.url.clone(), e.to_string())
        })?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HTTP Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SendResponse {
    pub msg_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusResponse {
    pub msg_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<String>,
}
