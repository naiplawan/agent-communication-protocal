//! ACP Security — HMAC-SHA256 signed-token creation/verification, mTLS support
//!
//! Token format (JWT-like, not JWT):
//! `Authorization: ACP-Token <base64(header)>.<base64(payload)>.<base64(sig)>`
//! - Header: `base64({"alg":"HS256","typ":"ACP"})`
//! - Payload: `base64({"iss","aud","exp","iat","msg_id","nonce"})`

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Fixed token header. Any other header is rejected outright.
const TOKEN_HEADER: &[u8] = br#"{"alg":"HS256","typ":"ACP"}"#;

/// The `auth_type` value selecting HMAC signed-token authentication.
pub const AUTH_TYPE_SIGNED_TOKEN: &str = "signed-token";

// ---------------------------------------------------------------------------
// Token Claims
// ---------------------------------------------------------------------------

/// Claims carried by a verified ACP token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPayload {
    /// Issuer, as `agent_id@machine_id`.
    pub iss: String,
    /// Audience, as `agent_id@machine_id`.
    pub aud: String,
    /// Expiry, RFC 3339.
    pub exp: String,
    /// Issue time, RFC 3339.
    pub iat: String,
    /// Message this token is bound to.
    pub msg_id: String,
    /// Random 128-bit value guarding against replay.
    pub nonce: String,
}

// ---------------------------------------------------------------------------
// Token Error
// ---------------------------------------------------------------------------

/// Why a token failed verification.
#[derive(Error, Debug)]
pub enum TokenError {
    /// The token was not three dot-separated parts, or a part was unreadable.
    #[error("Token must have 3 dot-separated parts")]
    Malformed,

    /// The header did not match the expected HS256/ACP value.
    #[error("Invalid header format — expected HS256/ACP")]
    InvalidHeader,

    /// A token part was not valid URL-safe base64.
    #[error("Invalid base64 in signature")]
    InvalidBase64,

    /// The signature did not match the expected HMAC.
    #[error("Signature mismatch")]
    SignatureMismatch,

    /// The token was minted for a different audience.
    #[error("Audience mismatch: got {0}, expected {1}")]
    AudienceMismatch(String, String),

    /// The payload had no `exp` claim.
    #[error("Missing exp claim")]
    MissingExp,

    /// The token's `exp` is in the past.
    #[error("Token expired at {0}")]
    Expired(String),

    /// The token was bound to a different message.
    #[error("msg_id mismatch: token bound to {token}, expected {expected}")]
    MsgIdMismatch {
        /// Message the token was minted for.
        token: String,
        /// Message it was presented against.
        expected: String,
    },
}

fn now_secs() -> i64 {
    Utc::now().timestamp()
}

// ---------------------------------------------------------------------------
// Base64 helpers
// ---------------------------------------------------------------------------

fn base64url_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

fn base64url_decode(input: &str) -> Result<Vec<u8>, TokenError> {
    URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|_| TokenError::InvalidBase64)
}

// ---------------------------------------------------------------------------
// Token Creation
// ---------------------------------------------------------------------------

/// Create an HMAC-SHA256 signed token bound to `msg_id`.
///
/// `secret` may be hex-encoded or raw: bytes are read as hex pairs, and a pair
/// that is not valid hex reads as zero, so both ends need only agree.
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
    ttl_seconds: i64,
) -> String {
    let now = Utc::now();
    let exp_time = DateTime::from_timestamp(now.timestamp() + ttl_seconds, 0)
        .unwrap_or(now)
        .to_rfc3339();

    let payload_dict = serde_json::json!({
        "iss": format!("{issuer_agent_id}@{issuer_machine_id}"),
        "aud": format!("{audience_agent_id}@{audience_machine_id}"),
        "exp": exp_time,
        "iat": now.to_rfc3339(),
        "msg_id": msg_id,
        "nonce": uuid::Uuid::new_v4().to_string(),
    });

    // A `serde_json::Value` built from string keys and string values always
    // serializes; the only documented failure is a map with non-string keys.
    let payload_bytes =
        serde_json::to_vec(&payload_dict).expect("json! value with string keys always serializes");

    let header_b64 = base64url_encode(TOKEN_HEADER);
    let payload_b64 = base64url_encode(&payload_bytes);
    let sig_b64 = base64url_encode(&sign(&format!("{header_b64}.{payload_b64}"), secret));

    format!("{header_b64}.{payload_b64}.{sig_b64}")
}

/// HMAC-SHA256 of `signing_input` under `secret`.
///
/// HMAC accepts a key of any length, so this cannot fail.
fn sign(signing_input: &str, secret: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(&hex_to_bytes(secret))
        .expect("HMAC-SHA256 accepts a key of any size");
    mac.update(signing_input.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Verify a signed token and return its claims.
///
/// Checks, in order: structure, header, signature, audience, expiry, and — when
/// `required_msg_id` is set — the message binding.
///
/// # Errors
/// Returns the [`TokenError`] naming the first check that failed.
pub fn verify_token(
    token: &str,
    secret: &str,
    expected_audience_agent_id: &str,
    expected_audience_machine_id: &str,
    required_msg_id: Option<&str>,
) -> Result<TokenPayload, TokenError> {
    let parts: Vec<&str> = token.split('.').collect();
    let [header_b64, payload_b64, sig_b64] = parts[..] else {
        return Err(TokenError::Malformed);
    };

    if base64url_decode(header_b64)? != TOKEN_HEADER {
        return Err(TokenError::InvalidHeader);
    }

    let expected_sig = sign(&format!("{header_b64}.{payload_b64}"), secret);
    if expected_sig != base64url_decode(sig_b64)? {
        return Err(TokenError::SignatureMismatch);
    }

    let payload_bytes = base64url_decode(payload_b64)?;
    let payload: serde_json::Value =
        serde_json::from_slice(&payload_bytes).map_err(|_| TokenError::Malformed)?;

    let aud = payload["aud"].as_str().ok_or(TokenError::Malformed)?;
    let expected_aud = format!("{expected_audience_agent_id}@{expected_audience_machine_id}");
    if aud != expected_aud {
        return Err(TokenError::AudienceMismatch(aud.to_string(), expected_aud));
    }

    let exp_str = payload["exp"].as_str().ok_or(TokenError::MissingExp)?;
    let exp = DateTime::parse_from_rfc3339(exp_str)
        .map_err(|_| TokenError::Malformed)?
        .timestamp();
    if exp < now_secs() {
        return Err(TokenError::Expired(exp_str.to_string()));
    }

    let token_msg_id = payload["msg_id"].as_str().ok_or(TokenError::Malformed)?;
    if let Some(required) = required_msg_id {
        if token_msg_id != required {
            return Err(TokenError::MsgIdMismatch {
                token: token_msg_id.to_string(),
                expected: required.to_string(),
            });
        }
    }

    Ok(TokenPayload {
        iss: payload["iss"].as_str().unwrap_or_default().to_string(),
        aud: aud.to_string(),
        exp: exp_str.to_string(),
        iat: payload["iat"].as_str().unwrap_or_default().to_string(),
        msg_id: token_msg_id.to_string(),
        nonce: payload["nonce"].as_str().unwrap_or_default().to_string(),
    })
}

// ---------------------------------------------------------------------------
// mTLS helpers
// ---------------------------------------------------------------------------

/// Certificate paths for a peer authenticated by mutual TLS.
#[derive(Debug, Clone)]
pub struct MTLSConfig {
    /// This agent's client certificate.
    pub cert_path: String,
    /// Private key for `cert_path`.
    pub key_path: String,
    /// CA certificate that signed the peer's certificate.
    pub verify_path: String,
}

// ---------------------------------------------------------------------------
// Peer Auth Config (loaded from acp-peers.yaml)
// ---------------------------------------------------------------------------

/// How this agent authenticates to one peer, as configured in `acp-peers.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PeerAuth {
    /// Either [`AUTH_TYPE_SIGNED_TOKEN`] or an mTLS scheme handled at the TLS layer.
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
    /// mTLS client certificate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_path: Option<String>,
    /// mTLS private key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    /// mTLS CA certificate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_path: Option<String>,
    /// Expected token issuer, when it differs from the peer's agent ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// File holding the shared signing secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_path: Option<String>,
}

fn default_auth_type() -> String {
    AUTH_TYPE_SIGNED_TOKEN.to_string()
}

impl PeerAuth {
    /// Signed-token authentication with no paths configured.
    #[must_use]
    pub fn new_signed_token() -> Self {
        Self {
            auth_type: default_auth_type(),
            cert_path: None,
            key_path: None,
            verify_path: None,
            issuer: None,
            secret_path: None,
        }
    }

    /// Read the signing secret from [`PeerAuth::secret_path`].
    ///
    /// Returns `None` for mTLS peers, when no path is configured, or when the
    /// file cannot be read.
    #[must_use]
    pub fn get_secret(&self) -> Option<String> {
        if self.auth_type != AUTH_TYPE_SIGNED_TOKEN {
            return None;
        }
        let secret_path = self.secret_path.as_ref()?;
        std::fs::read_to_string(secret_path)
            .ok()
            .map(|s| s.trim().to_string())
    }
}

impl Default for PeerAuth {
    fn default() -> Self {
        Self::new_signed_token()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Decode a hex secret into key bytes.
///
/// Non-hex pairs decode to zero rather than failing, so a raw (non-hex) secret
/// is still usable as a key as long as both ends read it the same way.
fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            let end = (i + 2).min(hex.len());
            u8::from_str_radix(&hex[i..end], 16).unwrap_or(0)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "abcd1234efgh5678";

    fn token_for(msg_id: &str, ttl_seconds: i64) -> String {
        create_token(
            "agent-alpha",
            "laptop-1",
            "agent-beta",
            "server-1",
            msg_id,
            TEST_SECRET,
            ttl_seconds,
        )
    }

    #[test]
    fn verified_token_carries_its_issuer_and_audience() {
        let token = token_for("msg_test123", 3600);

        let payload = verify_token(
            &token,
            TEST_SECRET,
            "agent-beta",
            "server-1",
            Some("msg_test123"),
        )
        .unwrap();

        assert_eq!(payload.iss, "agent-alpha@laptop-1");
        assert_eq!(payload.aud, "agent-beta@server-1");
    }

    #[test]
    fn verified_token_stays_bound_to_its_message() {
        let token = token_for("msg_test123", 3600);

        let payload = verify_token(&token, TEST_SECRET, "agent-beta", "server-1", None).unwrap();

        assert_eq!(payload.msg_id, "msg_test123");
    }

    #[test]
    fn token_signed_with_another_secret_is_rejected() {
        let token = token_for("msg_test123", 3600);

        let result = verify_token(&token, "wrong_secret", "agent-beta", "server-1", None);

        assert!(matches!(result, Err(TokenError::SignatureMismatch)));
    }

    #[test]
    fn token_presented_to_the_wrong_audience_is_rejected() {
        let token = token_for("msg_test123", 3600);

        let result = verify_token(&token, TEST_SECRET, "agent-gamma", "server-2", None);

        assert!(matches!(result, Err(TokenError::AudienceMismatch(_, _))));
    }

    #[test]
    fn token_presented_against_another_message_is_rejected() {
        let token = token_for("msg_test123", 3600);

        let result = verify_token(
            &token,
            TEST_SECRET,
            "agent-beta",
            "server-1",
            Some("msg_other"),
        );

        assert!(matches!(result, Err(TokenError::MsgIdMismatch { .. })));
    }

    #[test]
    fn expired_token_is_rejected() {
        let token = token_for("msg_test123", -10);

        let result = verify_token(&token, TEST_SECRET, "agent-beta", "server-1", None);

        assert!(matches!(result, Err(TokenError::Expired(_))));
    }

    #[test]
    fn token_without_three_parts_is_malformed() {
        let result = verify_token("one.two", TEST_SECRET, "a", "b", None);

        assert!(matches!(result, Err(TokenError::Malformed)));
    }

    #[test]
    fn peer_auth_trims_the_secret_file() {
        let dir = tempfile::tempdir().unwrap();
        let secret_file = dir.path().join("secret.key");
        std::fs::write(&secret_file, "my-secret-key\n  \n").unwrap();

        let auth = PeerAuth {
            secret_path: Some(secret_file.to_string_lossy().to_string()),
            ..Default::default()
        };

        assert_eq!(auth.get_secret(), Some("my-secret-key".to_string()));
    }

    #[test]
    fn peer_auth_without_a_secret_path_has_no_secret() {
        assert_eq!(PeerAuth::new_signed_token().get_secret(), None);
    }
}
