//! ACP Transport — HTTP client with signed-token auth + retry, WebSocket client

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{ACPConfig, Peer};
use crate::protocol::{
    build_ws_frame, new_ack_id, Message, StreamChunk, PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS,
};
use crate::security::{create_token, AUTH_TYPE_SIGNED_TOKEN};

/// Environment variable holding the fallback shared signing secret.
pub const SHARED_SECRET_ENV: &str = "ACP_SHARED_SECRET";

/// Wall-clock ceiling on a single HTTP request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why an ACP request did not complete.
#[derive(Error, Debug)]
pub enum TransportError {
    /// The peer answered with a non-success status.
    #[error("HTTP {status}: {message}")]
    HttpError {
        /// Status code returned by the peer.
        status: u16,
        /// Response body, when the peer sent one.
        message: String,
    },

    /// The peer could not be contacted, or gave up mid-exchange.
    #[error("Cannot reach {0}: {1}")]
    Unreachable(String, String),

    /// The peer rejected this agent's credentials.
    #[error("Auth failed for {0}: {1}")]
    SecurityError(String, String),

    /// The request outlived the 30-second request timeout.
    #[error("Timeout after {0}ms")]
    Timeout(u64),

    /// A request or frame could not be built or serialized.
    #[error("Transport error: {0}")]
    Transport(String),
}

// ---------------------------------------------------------------------------
// HTTP Client
// ---------------------------------------------------------------------------

/// HTTP client that signs every peer-addressed request with an ACP token.
#[derive(Clone)]
pub struct ACPHttpClient {
    config: Arc<ACPConfig>,
    this_agent_id: String,
    this_machine_id: String,
    http_client: reqwest::Client,
}

impl ACPHttpClient {
    /// Build a client that sends as `this_agent_id@this_machine_id`.
    #[must_use]
    pub fn new(config: ACPConfig, this_agent_id: String, this_machine_id: String) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            config: Arc::new(config),
            this_agent_id,
            this_machine_id,
            http_client,
        }
    }

    /// Mint an `Authorization` value for one request to `peer`.
    ///
    /// Returns `None` for mTLS peers — those authenticate at the TLS layer — and
    /// for signed-token peers with no reachable secret.
    fn build_auth_header(
        &self,
        peer: &Peer,
        msg_id: &str,
    ) -> Result<Option<String>, TransportError> {
        let auth = peer.auth.clone().unwrap_or_default();
        if auth.auth_type != AUTH_TYPE_SIGNED_TOKEN {
            return Ok(None);
        }

        let Some(secret) = auth.get_secret().or_else(|| {
            std::env::var(SHARED_SECRET_ENV)
                .ok()
                .filter(|s| !s.is_empty())
        }) else {
            return Err(TransportError::SecurityError(
                peer.agent_id.clone(),
                format!("No signing secret found; set {SHARED_SECRET_ENV} or configure peer auth"),
            ));
        };

        let token = create_token(
            &self.this_agent_id,
            &self.this_machine_id,
            &peer.agent_id,
            &peer.machine_id,
            msg_id,
            &secret,
            i64::try_from(self.config.security.token_ttl_seconds).unwrap_or(i64::MAX),
        );
        Ok(Some(format!("ACP-Token {token}")))
    }

    async fn do_request(
        &self,
        method: reqwest::Method,
        url: &str,
        peer: Option<&Peer>,
        msg_id: Option<&str>,
        json_body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, TransportError> {
        let mut req = self
            .http_client
            .request(method, url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("User-Agent", "acp-rust/0.1");

        if let (Some(peer), Some(msg_id)) = (peer, msg_id) {
            if let Some(auth_header) = self.build_auth_header(peer, msg_id)? {
                req = req.header("Authorization", auth_header);
            }
        }

        if let Some(body) = json_body {
            req = req.json(&body);
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                TransportError::Timeout(
                    u64::try_from(REQUEST_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                )
            } else {
                TransportError::Unreachable(url.to_string(), e.to_string())
            }
        })?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(TransportError::SecurityError(
                peer.map(|p| p.agent_id.clone()).unwrap_or_default(),
                body,
            ));
        }

        if !status.is_success() {
            return Err(TransportError::HttpError {
                status: status.as_u16(),
                message: body,
            });
        }

        if body.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(&body).map_err(|e| TransportError::HttpError {
            status: status.as_u16(),
            message: format!("JSON parse error: {e}"),
        })
    }

    // ---- Message operations ----

    /// Negotiate the ACP version and features supported by `peer`.
    ///
    /// The request is authenticated and its token is bound to the literal
    /// `initialize` request name rather than to a message ID.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the peer cannot be reached, rejects
    /// the handshake, or returns an invalid initialization response.
    pub async fn initialize(
        &self,
        peer: &Peer,
        capabilities: &[&str],
    ) -> Result<InitializeResponse, TransportError> {
        let url = format!("{}/acp/v1/initialize", peer.http_endpoint);
        let body = serde_json::json!({
            "protocol_versions": SUPPORTED_PROTOCOL_VERSIONS,
            "capabilities": capabilities,
        });
        let value = self
            .do_request(
                reqwest::Method::POST,
                &url,
                Some(peer),
                Some("initialize"),
                Some(body),
            )
            .await?;
        serde_json::from_value(value).map_err(|error| {
            TransportError::Transport(format!(
                "Invalid initialize response for protocol {PROTOCOL_VERSION}: {error}"
            ))
        })
    }

    /// `POST /acp/v1/messages/send`.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the message cannot be serialized, the
    /// peer is unreachable, or it answers with a non-success status.
    pub async fn send_message(
        &self,
        peer: &Peer,
        message: &Message,
    ) -> Result<serde_json::Value, TransportError> {
        let url = format!("{}/acp/v1/messages/send", peer.http_endpoint);
        let body = serde_json::to_value(message)
            .map_err(|e| TransportError::Transport(format!("JSON serialize error: {e}")))?;
        self.do_request(
            reqwest::Method::POST,
            &url,
            Some(peer),
            Some(&message.envelope.msg_id),
            Some(body),
        )
        .await
    }

    /// `GET /acp/v1/messages/{msg_id}/status`.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the peer is unreachable or refuses.
    pub async fn get_message_status(
        &self,
        peer: &Peer,
        msg_id: &str,
    ) -> Result<serde_json::Value, TransportError> {
        let url = format!("{}/messages/{}/status", peer.http_endpoint, msg_id);
        self.do_request(reqwest::Method::GET, &url, Some(peer), Some(msg_id), None)
            .await
    }

    /// `POST /acp/v1/messages/{msg_id}/ack`.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the peer is unreachable or refuses.
    pub async fn ack_message(
        &self,
        peer: &Peer,
        msg_id: &str,
        ack: MessageAck<'_>,
    ) -> Result<serde_json::Value, TransportError> {
        let url = format!("{}/messages/{}/ack", peer.http_endpoint, msg_id);
        let body = serde_json::json!({
            "ack_id": new_ack_id(),
            "ack_type": ack.ack_type,
            "received": ack.received,
            "processed": ack.processed,
            "stream_available": ack.stream_available,
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

    /// `POST /acp/v1/messages/{msg_id}/error`.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the peer is unreachable or refuses.
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
        self.do_request(
            reqwest::Method::POST,
            &url,
            Some(peer),
            Some(msg_id),
            Some(body),
        )
        .await
    }

    /// `GET /acp/v1/messages/pending`.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the peer is unreachable or refuses.
    pub async fn poll_pending(&self, peer: &Peer) -> Result<serde_json::Value, TransportError> {
        let url = format!("{}/acp/v1/messages/pending", peer.http_endpoint);
        self.do_request(
            reqwest::Method::GET,
            &url,
            Some(peer),
            Some("poll_pending"),
            None,
        )
        .await
    }

    // ---- Stream operations ----

    /// `POST /acp/v1/stream/init`.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the peer is unreachable or refuses.
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

    /// Send a message, retrying transport failures with exponential backoff.
    ///
    /// Auth failures are not retried — a rejected token will be rejected again.
    ///
    /// # Errors
    /// Returns the last [`TransportError`] once `retry.max_attempts` is spent.
    pub async fn send_with_retry(
        &self,
        peer: &Peer,
        message: &Message,
    ) -> Result<serde_json::Value, TransportError> {
        let cfg = &self.config.retry;
        let mut backoff = Duration::from_millis(cfg.initial_backoff_ms);
        let max_backoff = Duration::from_millis(cfg.max_backoff_ms);

        for attempt in 1..=cfg.max_attempts {
            match self.send_message(peer, message).await {
                Ok(resp) => return Ok(resp),
                Err(
                    error @ (TransportError::Unreachable(..)
                    | TransportError::Timeout(_)
                    | TransportError::HttpError { .. }),
                ) => {
                    if attempt == cfg.max_attempts {
                        return Err(error);
                    }
                    tokio::time::sleep(backoff).await;
                    backoff = backoff.mul_f64(cfg.backoff_multiplier).min(max_backoff);
                }
                Err(error) => return Err(error),
            }
        }

        Err(TransportError::Unreachable(
            peer.agent_id.clone(),
            format!("Max retries ({}) exceeded", cfg.max_attempts),
        ))
    }
}

/// What one acknowledgement confirms about a message.
#[derive(Debug, Clone, Copy)]
pub struct MessageAck<'a> {
    /// Which stage is being confirmed, e.g. `"hop_ack"`.
    pub ack_type: &'a str,
    /// The recipient has the message.
    pub received: bool,
    /// The recipient has finished handling it.
    pub processed: bool,
    /// A streamed reply is available for it.
    pub stream_available: bool,
}

impl<'a> MessageAck<'a> {
    /// Confirm receipt only.
    #[must_use]
    pub fn hop(ack_type: &'a str) -> Self {
        Self {
            ack_type,
            received: true,
            processed: false,
            stream_available: false,
        }
    }

    /// Confirm receipt and completed handling.
    #[must_use]
    pub fn processed(ack_type: &'a str) -> Self {
        Self {
            ack_type,
            received: true,
            processed: true,
            stream_available: false,
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket Client
// ---------------------------------------------------------------------------

/// One frame read off a stream: its header and its body.
#[derive(Debug, Clone)]
pub struct RawStreamFrame {
    /// The `frame` object, or `null` when the peer omitted it.
    pub frame: serde_json::Value,
    /// The `data` object, or `null` when the peer omitted it.
    pub data: serde_json::Value,
}

/// WebSocket client for streamed ACP replies.
pub struct ACPWebSocketClient {
    url: String,
    inner: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl ACPWebSocketClient {
    /// Open a stream to `url`, presenting `token` when one is given.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when `url` is not a valid request target or
    /// the handshake fails.
    pub async fn connect(url: &str, token: Option<String>) -> Result<Self, TransportError> {
        let request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(url)
            .header("User-Agent", "acp-rust/0.1")
            .header(
                "Authorization",
                token.map(|t| format!("ACP-Token {t}")).unwrap_or_default(),
            )
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

    /// Send an already-built frame.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the frame cannot be serialized or the
    /// stream has closed.
    pub async fn send_frame(&mut self, frame: serde_json::Value) -> Result<(), TransportError> {
        let text = serde_json::to_string(&frame)
            .map_err(|e| TransportError::Transport(format!("JSON serialize error: {e}")))?;

        self.inner
            .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
            .await
            .map_err(|e| TransportError::Unreachable(self.url.clone(), e.to_string()))?;

        Ok(())
    }

    /// Build and send one chunk of a stream.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the stream has closed.
    pub async fn send_chunk(&mut self, chunk: StreamChunk<'_>) -> Result<(), TransportError> {
        self.send_frame(build_ws_frame(chunk)).await
    }

    /// Read the next frame off the stream.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the stream closes, yields a non-text
    /// message, or yields text that is not JSON.
    pub async fn recv_frame(&mut self) -> Result<RawStreamFrame, TransportError> {
        let msg = match self.inner.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => return Err(TransportError::Transport(e.to_string())),
            None => {
                return Err(TransportError::Unreachable(
                    self.url.clone(),
                    "WebSocket closed".to_string(),
                ))
            }
        };

        let text = msg
            .to_text()
            .map_err(|e| TransportError::Transport(format!("WebSocket text error: {e}")))?;

        let raw: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| TransportError::Transport(format!("JSON parse error: {e}")))?;

        Ok(RawStreamFrame {
            frame: raw.get("frame").cloned().unwrap_or(serde_json::Value::Null),
            data: raw.get("data").cloned().unwrap_or(serde_json::Value::Null),
        })
    }

    /// Close the stream.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the close frame cannot be delivered.
    pub async fn close(mut self) -> Result<(), TransportError> {
        self.inner
            .close(None)
            .await
            .map_err(|e| TransportError::Unreachable(self.url.clone(), e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// HTTP Response types
// ---------------------------------------------------------------------------

/// Body returned by `POST /acp/v1/messages/send`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SendResponse {
    /// Message the response is about.
    pub msg_id: String,
    /// What the recipient did with it, e.g. `"accepted"` or `"brokered"`.
    pub status: String,
    /// Endpoint the message was forwarded to, when it was forwarded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,
    /// Why the message was refused, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Body returned by `GET /acp/v1/messages/{msg_id}/status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusResponse {
    /// Message the response is about.
    pub msg_id: String,
    /// Where the message stands.
    pub status: String,
    /// When it reached its recipient, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<String>,
}

/// Body returned by `POST /acp/v1/initialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InitializeResponse {
    /// Version selected for this connection.
    pub protocol_version: String,
    /// Highest version the peer supports.
    pub server_protocol_version: String,
    /// Versions the peer accepts on future initializations.
    #[serde(default)]
    pub supported_protocol_versions: Vec<String>,
    /// Peer role, such as `agent` or `relay`.
    pub role: String,
    /// Logical peer ID.
    pub agent_id: String,
    /// Peer machine ID.
    pub machine_id: String,
    /// Peer-declared capabilities.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Requested features accepted by the peer.
    #[serde(default)]
    pub accepted_capabilities: Vec<String>,
    /// Features the peer can use with this protocol version.
    #[serde(default)]
    pub features: Vec<String>,
    /// Message intents accepted by the peer.
    #[serde(default)]
    pub intents: Vec<String>,
    /// Payload MIME types accepted by the peer.
    #[serde(default)]
    pub content_types: Vec<String>,
    /// Authentication schemes accepted by the peer.
    #[serde(default)]
    pub auth: Vec<String>,
}
