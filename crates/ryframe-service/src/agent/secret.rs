use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use ryframe_kernel::{AppError, AppResult};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub(super) const CREDENTIAL_MAC_DOMAIN: &[u8] = b"ryframe/service-credential/v1\0";
pub(super) const DELEGATION_MAC_DOMAIN: &[u8] = b"ryframe/service-delegation/v1\0";
pub(super) const IP_DIGEST_DOMAIN: &[u8] = b"ryframe/agent-ip/v1\0";
pub(super) const USER_AGENT_DIGEST_DOMAIN: &[u8] = b"ryframe/agent-user-agent/v1\0";

pub(super) struct ParsedApiKey {
    pub key_id: String,
    presented_mac_input: Vec<u8>,
}

impl ParsedApiKey {
    pub fn verify(&self, pepper: &[u8], expected_mac: &[u8]) -> AppResult<bool> {
        verify_mac(
            pepper,
            CREDENTIAL_MAC_DOMAIN,
            &self.presented_mac_input,
            expected_mac,
        )
    }
}

pub(super) struct ParsedDelegation {
    token: String,
}

impl ParsedDelegation {
    pub fn mac(&self, pepper: &[u8]) -> AppResult<Vec<u8>> {
        keyed_hash(pepper, DELEGATION_MAC_DOMAIN, self.token.as_bytes())
    }

    pub fn verify(&self, pepper: &[u8], expected_mac: &[u8]) -> AppResult<bool> {
        verify_mac(
            pepper,
            DELEGATION_MAC_DOMAIN,
            self.token.as_bytes(),
            expected_mac,
        )
    }
}

pub(super) fn parse_authorization(value: Option<&str>) -> AppResult<ParsedApiKey> {
    let value = value.ok_or_else(invalid_credential)?;
    let (scheme, credential) = value.split_once(' ').ok_or_else(invalid_credential)?;
    if scheme != "RyFrameApiKey"
        || credential.contains(char::is_whitespace)
        || credential.matches('.').count() != 1
    {
        return Err(invalid_credential());
    }
    let credential = credential
        .strip_prefix("rfk_")
        .ok_or_else(invalid_credential)?;
    let (key_id, secret) = credential.split_once('.').ok_or_else(invalid_credential)?;
    validate_canonical_base64url(key_id, 18)?;
    validate_canonical_base64url(secret, 32)?;
    Ok(ParsedApiKey {
        key_id: key_id.to_owned(),
        // 管理域创建凭据时对完整展示值计算 MAC，校验必须使用完全相同的规范字节串。
        presented_mac_input: value
            .strip_prefix("RyFrameApiKey ")
            .expect("认证方案已经完成精确校验")
            .as_bytes()
            .to_vec(),
    })
}

pub(super) fn parse_delegation(value: Option<&str>) -> AppResult<Option<ParsedDelegation>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.contains(char::is_whitespace) {
        return Err(invalid_credential());
    }
    let secret = value.strip_prefix("rfd_").ok_or_else(invalid_credential)?;
    validate_canonical_base64url(secret, 32)?;
    Ok(Some(ParsedDelegation {
        token: value.to_owned(),
    }))
}

fn validate_canonical_base64url(value: &str, expected_bytes: usize) -> AppResult<()> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value.as_bytes())
        .map_err(|_| invalid_credential())?;
    if decoded.len() != expected_bytes || URL_SAFE_NO_PAD.encode(decoded) != value {
        return Err(invalid_credential());
    }
    Ok(())
}

pub(super) fn keyed_hash(key: &[u8], domain: &[u8], value: &[u8]) -> AppResult<Vec<u8>> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|_| AppError::Config("Pepper 长度无效".into()))?;
    mac.update(domain);
    mac.update(value);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn verify_mac(key: &[u8], domain: &[u8], value: &[u8], expected: &[u8]) -> AppResult<bool> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|_| AppError::Config("Pepper 长度无效".into()))?;
    mac.update(domain);
    mac.update(value);
    Ok(mac.verify_slice(expected).is_ok())
}

pub(super) fn invalid_credential() -> AppError {
    AppError::Authentication("Agent 凭据无效".into())
}
