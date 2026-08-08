//! ACP Security — HMAC-SHA256 signed-token creation/verification, mTLS support
//!
//! Token format (JWT-like, not JWT):
//! Authorization: ACP-Token <base64(header)>.<base64(payload)>.<base64(sig)>
//! Header: base64({"alg":"HS256","typ":"ACP"})
//! Payload: base64({"iss","aud","exp","iat","msg_id","nonce"})

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use chrono::{DateTime, Utc};

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Token Claims
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPayload {
    pub iss: String,  // issuer: agent_id@machine_id
    pub aud: String,  // audience: agent_id@machine_id
    pub exp: String,  // expiration ISO-8601
    pub iat: String,  // issued at ISO-8601
    pub msg_id: String,  // binds token to specific message
    pub nonce: String,  // random 128-bit, anti-replay
}

// ---------------------------------------------------------------------------
// Token Error
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum TokenError {
    #[error("Token must have 3 dot-separated parts")]
    Malformed,

    #[error("Invalid header format — expected HS256/ACP")]
    InvalidHeader,

    #[error("Invalid base64 in signature")]
    InvalidBase64,

    #[error("Signature mismatch")]
    SignatureMismatch,

    #[error("Audience mismatch: got {0}, expected {1}")]
    AudienceMismatch(String, String),

    #[error("Missing exp claim")]
    MissingExp,

    #[error("Token expired at {0}")]
    Expired(String),

    #[error("msg_id mismatch: token bound to {token}, expected {expected}")]
    MsgIdMismatch { token: String, expected: String },
}

fn now_secs() -> i64 {
    Utc::now()
        .timestamp()
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

/// Create an HMAC-SHA256 signed token
///
/// `secret` can be hex-encoded ("ab12...") or raw bytes
pub fn create_token(
    issuer_agent_id: &str,
    issuer_machine_id: &str,
    audience_agent_id: &str,
    audience_machine_id: &str,
    msg_id: &str,
    secret: &str,
    ttl_seconds: i64,
) -> String {
    let iss = format!("{}@{}", issuer_agent_id, issuer_machine_id);
    let aud = format!("{}@{}", audience_agent_id, audience_machine_id);

    let now = Utc::now();
    let iat = now.to_rfc3339();
    let exp_time = DateTime::from_timestamp(now.timestamp() + ttl_seconds, 0)
        .unwrap_or(now)
        .to_rfc3339();

    let nonce = uuid::Uuid::new_v4().to_string();

    let payload_dict = serde_json::json!({
        "iss": iss,
        "aud": aud,
        "exp": exp_time,
        "iat": iat,
        "msg_id": msg_id,
        "nonce": nonce,
    });

    let header_bytes = b"{\"alg\":\"HS256\",\"typ\":\"ACP\"}";
    let payload_bytes = serde_json::to_vec(&payload_dict).unwrap();

    let header_b64 = base64url_encode(header_bytes);
    let payload_b64 = base64url_encode(&payload_bytes);

    let signing_input = format!("{}.{}", header_b64, payload_b64);
    let key = hex_to_bytes(secret);

    let mut mac = HmacSha256::new_from_slice(&key).expect("HMAC can take any key size");
    mac.update(signing_input.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = base64url_encode(sig.as_slice());

    format!("{}.{}.{}", header_b64, payload_b64, sig_b64)
}

/// Verify a signed token
///
/// Returns `TokenPayload` on success
pub fn verify_token(
    token: &str,
    secret: &str,
    expected_audience_agent_id: &str,
    expected_audience_machine_id: &str,
    required_msg_id: Option<&str>,
) -> Result<TokenPayload, TokenError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(TokenError::Malformed);
    }

    let header_b64 = parts[0];
    let payload_b64 = parts[1];
    let sig_b64 = parts[2];

    // Decode header and verify
    let header_bytes = base64url_decode(header_b64)?;
    if &header_bytes != b"{\"alg\":\"HS256\",\"typ\":\"ACP\"}" {
        return Err(TokenError::InvalidHeader);
    }

    // Verify signature
    let signing_input = format!("{}.{}", header_b64, payload_b64);
    let key = hex_to_bytes(secret);

    let mut mac = HmacSha256::new_from_slice(&key).map_err(|_| TokenError::Malformed)?;
    mac.update(signing_input.as_bytes());

    let expected_sig = mac.finalize().into_bytes();
    let actual_sig = base64url_decode(sig_b64)?;

    if expected_sig.as_slice() != actual_sig.as_slice() {
        return Err(TokenError::SignatureMismatch);
    }

    // Decode payload
    let payload_bytes = base64url_decode(payload_b64)?;
    let payload: serde_json::Value =
        serde_json::from_slice(&payload_bytes).map_err(|_| TokenError::Malformed)?;

    // Verify audience
    let aud = payload["aud"]
        .as_str()
        .ok_or(TokenError::Malformed)?;
    let expected_aud = format!("{}@{}", expected_audience_agent_id, expected_audience_machine_id);
    if aud != expected_aud {
        return Err(TokenError::AudienceMismatch(aud.to_string(), expected_aud));
    }

    // Verify expiration
    let exp_str = payload["exp"].as_str().ok_or(TokenError::MissingExp)?;
    let exp = DateTime::parse_from_rfc3339(exp_str)
        .map_err(|_| TokenError::Malformed)?
        .timestamp();

    if exp < now_secs() {
        return Err(TokenError::Expired(exp_str.to_string()));
    }

    // Verify msg_id binding
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
        iss: payload["iss"].as_str().unwrap_or("").to_string(),
        aud: aud.to_string(),
        exp: exp_str.to_string(),
        iat: payload["iat"].as_str().unwrap_or("").to_string(),
        msg_id: token_msg_id.to_string(),
        nonce: payload["nonce"].as_str().unwrap_or("").to_string(),
    })
}

// ---------------------------------------------------------------------------
// mTLS helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MTLSConfig {
    pub cert_path: String,
    pub key_path: String,
    pub verify_path: String,  // CA cert that signed the peer's cert
}

// ---------------------------------------------------------------------------
// Peer Auth Config (loaded from acp-peers.yaml)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PeerAuth {
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_path: Option<String>,
}

fn default_auth_type() -> String {
    "signed-token".to_string()
}

impl PeerAuth {
    pub fn new_signed_token() -> Self {
        Self {
            auth_type: "signed-token".to_string(),
            cert_path: None,
            key_path: None,
            verify_path: None,
            issuer: None,
            secret_path: None,
        }
    }

    /// Read the signing secret from the secret path file
    pub fn get_secret(&self) -> Option<String> {
        if let (Some(ref secret_path), "signed-token") = (&self.secret_path, self.auth_type.as_str()) {
            std::fs::read_to_string(secret_path).ok().map(|s| s.trim().to_string())
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

    #[test]
    fn test_create_and_verify_token() {
        let token = create_token(
            "agent-alpha", "laptop-1",
            "agent-beta", "server-1",
            "msg_test123",
            TEST_SECRET,
            3600,
        );

        let payload = verify_token(
            &token,
            TEST_SECRET,
            "agent-beta", "server-1",
            Some("msg_test123"),
        )
        .unwrap();

        assert_eq!(payload.iss, "agent-alpha@laptop-1");
        assert_eq!(payload.aud, "agent-beta@server-1");
        assert_eq!(payload.msg_id, "msg_test123");
    }

    #[test]
    fn test_token_wrong_secret() {
        let token = create_token(
            "agent-alpha", "laptop-1",
            "agent-beta", "server-1",
            "msg_test123",
            TEST_SECRET,
            3600,
        );

        let result = verify_token(
            &token,
            "wrong_secret",
            "agent-beta", "server-1",
            None,
        );
        assert!(matches!(result, Err(TokenError::SignatureMismatch)));
    }

    #[test]
    fn test_token_wrong_audience() {
        let token = create_token(
            "agent-alpha", "laptop-1",
            "agent-beta", "server-1",
            "msg_test123",
            TEST_SECRET,
            3600,
        );

        let result = verify_token(
            &token,
            TEST_SECRET,
            "agent-gamma", "server-2",
            None,
        );
        assert!(matches!(result, Err(TokenError::AudienceMismatch(_, _))));
    }

    #[test]
    fn test_token_expired() {
        let token = create_token(
            "agent-alpha", "laptop-1",
            "agent-beta", "server-1",
            "msg_test123",
            TEST_SECRET,
            -10,  // expired 10 seconds ago
        );

        let result = verify_token(
            &token,
            TEST_SECRET,
            "agent-beta", "server-1",
            None,
        );
        assert!(matches!(result, Err(TokenError::Expired(_))));
    }

    #[test]
    fn test_malformed_token() {
        assert!(matches!(
            verify_token("not.valid", TEST_SECRET, "a", "b", None),
            Err(TokenError::Malformed)
        ));
        assert!(matches!(
            verify_token("one.two", TEST_SECRET, "a", "b", None),
            Err(TokenError::Malformed)
        ));
    }

    #[test]
    fn test_peer_auth_get_secret() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let secret_file = dir.path().join("secret.key");
        std::fs::write(&secret_file, "my-secret-key\n  \n").unwrap();

        let auth = PeerAuth {
            auth_type: "signed-token".to_string(),
            secret_path: Some(secret_file.to_string_lossy().to_string()),
            ..Default::default()
        };
        assert_eq!(auth.get_secret(), Some("my-secret-key".to_string()));
    }

    impl Default for PeerAuth {
        fn default() -> Self {
            Self::new_signed_token()
        }
    }
}
