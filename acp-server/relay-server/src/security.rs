//! HMAC-SHA256 token verification — the relay half of the scheme in `acp-core`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use thiserror::Error;

use crate::models::TokenClaims;

type HmacSha256 = Hmac<sha2::Sha256>;

/// Fixed token header. Any other header is rejected outright.
const TOKEN_HEADER: &[u8] = br#"{"alg":"HS256","typ":"ACP"}"#;

/// Why a token failed verification.
#[derive(Error, Debug)]
pub enum TokenError {
    /// The header did not match the expected HS256/ACP value.
    #[error("Invalid header format")]
    InvalidHeader,
    /// The token was not three readable base64 parts, or a claim was missing.
    #[error("Malformed token")]
    Malformed,
    /// The signature did not match the expected HMAC.
    #[error("Invalid signature")]
    InvalidSignature,
    /// The token's `exp` is in the past.
    #[error("Token expired")]
    Expired,
}

fn b64_decode(input: &str) -> Result<Vec<u8>, TokenError> {
    URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|_| TokenError::Malformed)
}

/// Convert an even-length hexadecimal secret to bytes; other secrets are used
/// as raw UTF-8 bytes.
pub(crate) fn secret_bytes(secret: &str) -> Vec<u8> {
    let bytes = secret.as_bytes();
    if !bytes.len().is_multiple_of(2) || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return bytes.to_vec();
    }

    bytes
        .chunks_exact(2)
        .map(|pair| (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]))
        .collect()
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => unreachable!("hex_nibble only receives hexadecimal input"),
    }
}

/// Verify a signed token and return its claims.
///
/// Checks structure, header, signature, and expiry. The audience is *not*
/// checked here — the relay accepts tokens addressed either to itself or to the
/// ultimate recipient, so each handler decides which is acceptable.
///
/// # Errors
/// Returns the [`TokenError`] naming the first check that failed.
pub fn verify_token(token: &str, secret: &str) -> Result<TokenClaims, TokenError> {
    if secret.is_empty() {
        return Err(TokenError::Malformed);
    }
    let parts: Vec<&str> = token.split('.').collect();
    let [header_b64, payload_b64, sig_b64] = parts[..] else {
        return Err(TokenError::Malformed);
    };

    if b64_decode(header_b64)? != TOKEN_HEADER {
        return Err(TokenError::InvalidHeader);
    }

    let mut mac =
        HmacSha256::new_from_slice(&secret_bytes(secret)).map_err(|_| TokenError::Malformed)?;
    mac.update(format!("{header_b64}.{payload_b64}").as_bytes());
    if mac.verify_slice(&b64_decode(sig_b64)?).is_err() {
        return Err(TokenError::InvalidSignature);
    }

    let payload: serde_json::Value =
        serde_json::from_slice(&b64_decode(payload_b64)?).map_err(|_| TokenError::Malformed)?;

    let claim = |name: &str| {
        payload[name]
            .as_str()
            .map(String::from)
            .ok_or(TokenError::Malformed)
    };

    let issuer = claim("iss")?;
    if issuer.is_empty() {
        return Err(TokenError::Malformed);
    }
    let nonce = claim("nonce")?;
    if nonce.is_empty() {
        return Err(TokenError::Malformed);
    }
    let msg_id = claim("msg_id")?;
    if msg_id.is_empty() {
        return Err(TokenError::Malformed);
    }
    let exp_str = claim("exp")?;
    let exp = chrono::DateTime::parse_from_rfc3339(&exp_str)
        .map_err(|_| TokenError::Malformed)?
        .timestamp();
    let iat = chrono::DateTime::parse_from_rfc3339(&claim("iat")?)
        .map_err(|_| TokenError::Malformed)?
        .timestamp();
    let now = chrono::Utc::now().timestamp();
    if iat > now + 60 || exp < iat {
        return Err(TokenError::Malformed);
    }
    if exp < now {
        return Err(TokenError::Expired);
    }

    Ok(TokenClaims {
        iss: issuer,
        sub: claim("aud")?,
        msg_id,
        exp,
    })
}

/// Take the `agent_id` half of an `agent_id@machine_id` address.
#[must_use]
pub fn extract_agent_id(addr: &str) -> String {
    addr.split('@').next().unwrap_or(addr).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::create_token;

    const TEST_SECRET: &str = "abcd1234efgh5678";

    #[test]
    fn a_relay_signed_token_verifies() {
        let token = create_token(
            "acp-relay",
            "relay",
            "agent-beta",
            "server-1",
            "msg_1",
            TEST_SECRET,
        );

        let claims = verify_token(&token, TEST_SECRET).unwrap();

        assert_eq!(claims.iss, "acp-relay@relay");
    }

    #[test]
    fn a_verified_token_keeps_its_audience() {
        let token = create_token(
            "acp-relay",
            "relay",
            "agent-beta",
            "server-1",
            "msg_1",
            TEST_SECRET,
        );

        let claims = verify_token(&token, TEST_SECRET).unwrap();

        assert_eq!(claims.sub, "agent-beta@server-1");
    }

    #[test]
    fn a_token_signed_with_another_secret_is_rejected() {
        let token = create_token("a", "m", "b", "m", "msg_1", TEST_SECRET);

        let error = verify_token(&token, "0badsecret").unwrap_err();

        assert!(matches!(error, TokenError::InvalidSignature));
    }

    #[test]
    fn a_token_without_three_parts_is_malformed() {
        let error = verify_token("one.two", TEST_SECRET).unwrap_err();

        assert!(matches!(error, TokenError::Malformed));
    }

    #[test]
    fn an_odd_length_secret_does_not_panic() {
        let token = create_token("a", "m", "b", "m", "msg_1", "abc");

        assert!(verify_token(&token, "abc").is_ok());
    }

    #[test]
    fn extract_agent_id_takes_the_half_before_the_at() {
        assert_eq!(extract_agent_id("agent-beta@server-1"), "agent-beta");
    }

    #[test]
    fn extract_agent_id_passes_through_a_bare_agent() {
        assert_eq!(extract_agent_id("agent-beta"), "agent-beta");
    }
}
