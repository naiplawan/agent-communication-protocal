//! HMAC-SHA256 token verification — matches Python lib/security.py

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TokenError {
    #[error("Invalid header format")]
    InvalidHeader,
    #[error("Malformed token")]
    Malformed,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Token expired")]
    Expired,
}

fn b64_decode(input: &str) -> Result<Vec<u8>, TokenError> {
    URL_SAFE_NO_PAD.decode(input).map_err(|_| TokenError::Malformed)
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0))
        .collect()
}

pub fn verify_token(token: &str, secret: &str) -> Result<super::models::TokenClaims, TokenError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(TokenError::Malformed);
    }

    let header_bytes = b64_decode(parts[0])?;
    let payload_bytes = b64_decode(parts[1])?;
    let sig_bytes = b64_decode(parts[2])?;

    if &*header_bytes != b"{\"alg\":\"HS256\",\"typ\":\"ACP\"}" {
        return Err(TokenError::InvalidHeader);
    }

    let payload: serde_json::Value =
        serde_json::from_slice(&payload_bytes).map_err(|_| TokenError::Malformed)?;

    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let key = hex_to_bytes(secret);

    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;
    let mut mac = HmacSha256::new_from_slice(&key).map_err(|_| TokenError::Malformed)?;
    mac.update(signing_input.as_bytes());
    let expected = mac.finalize().into_bytes();

    if expected.as_slice() != sig_bytes {
        return Err(TokenError::InvalidSignature);
    }

    let iss = payload["iss"].as_str().ok_or(TokenError::Malformed)?.to_string();
    let sub = payload["aud"].as_str().ok_or(TokenError::Malformed)?.to_string();
    let msg_id = payload["msg_id"].as_str().ok_or(TokenError::Malformed)?.to_string();

    let exp_str = payload["exp"].as_str().ok_or(TokenError::Malformed)?;
    let exp = chrono::DateTime::parse_from_rfc3339(exp_str)
        .map_err(|_| TokenError::Malformed)?
        .timestamp();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    if exp < now {
        return Err(TokenError::Expired);
    }

    Ok(super::models::TokenClaims { iss, sub, msg_id, exp })
}

pub fn extract_agent_id(iss: &str) -> String {
    iss.split('@').next().unwrap_or(iss).to_string()
}
