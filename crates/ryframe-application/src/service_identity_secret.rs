use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use rand::RngExt as _;
use ryframe_kernel::{AppError, AppResult};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const API_KEY_PREFIX: &str = "rfk_";
const DELEGATION_TOKEN_PREFIX: &str = "rfd_";
const API_KEY_ID_BYTES: usize = 18;
const SECRET_BYTES: usize = 32;

pub(crate) const CREDENTIAL_MAC_DOMAIN: &[u8] = b"ryframe/service-credential/v1\0";
pub(crate) const DELEGATION_MAC_DOMAIN: &[u8] = b"ryframe/service-delegation/v1\0";
pub(crate) const IP_DIGEST_DOMAIN: &[u8] = b"ryframe/agent-ip/v1\0";
pub(crate) const USER_AGENT_DIGEST_DOMAIN: &[u8] = b"ryframe/agent-user-agent/v1\0";

/// 一次性签发的 API Key；刻意不实现 `Debug`，避免安全材料进入诊断输出。
pub(crate) struct IssuedApiKey {
    key_id: String,
    presented: String,
}

impl IssuedApiKey {
    pub fn issue() -> Self {
        let key_id = random_token(API_KEY_ID_BYTES);
        let secret = random_token(SECRET_BYTES);
        let presented = format!("{API_KEY_PREFIX}{key_id}.{secret}");
        Self { key_id, presented }
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn mac(&self, pepper: &[u8]) -> AppResult<Vec<u8>> {
        keyed_hash(pepper, CREDENTIAL_MAC_DOMAIN, self.presented.as_bytes())
    }

    pub fn into_presented(self) -> String {
        self.presented
    }
}

/// 一次性签发的委托令牌；刻意不实现 `Debug`，避免安全材料进入诊断输出。
pub(crate) struct IssuedDelegationToken {
    presented: String,
}

impl IssuedDelegationToken {
    pub fn issue() -> Self {
        Self {
            presented: format!("{DELEGATION_TOKEN_PREFIX}{}", random_token(SECRET_BYTES)),
        }
    }

    pub fn mac(&self, pepper: &[u8]) -> AppResult<Vec<u8>> {
        keyed_hash(pepper, DELEGATION_MAC_DOMAIN, self.presented.as_bytes())
    }

    pub fn into_presented(self) -> String {
        self.presented
    }
}

pub(crate) struct ParsedApiKey {
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

pub(crate) struct ParsedDelegation {
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

pub(crate) fn parse_authorization(value: Option<&str>) -> AppResult<ParsedApiKey> {
    let value = value.ok_or_else(invalid_credential)?;
    let (scheme, credential) = value.split_once(' ').ok_or_else(invalid_credential)?;
    if scheme != "RyFrameApiKey"
        || credential.contains(char::is_whitespace)
        || credential.matches('.').count() != 1
    {
        return Err(invalid_credential());
    }
    let credential = credential
        .strip_prefix(API_KEY_PREFIX)
        .ok_or_else(invalid_credential)?;
    let (key_id, secret) = credential.split_once('.').ok_or_else(invalid_credential)?;
    validate_canonical_base64url(key_id, API_KEY_ID_BYTES)?;
    validate_canonical_base64url(secret, SECRET_BYTES)?;
    let presented = value
        .strip_prefix("RyFrameApiKey ")
        .ok_or_else(invalid_credential)?;
    Ok(ParsedApiKey {
        key_id: key_id.to_owned(),
        // 签发凭据时对完整展示值计算 MAC，校验必须使用完全相同的规范字节串。
        presented_mac_input: presented.as_bytes().to_vec(),
    })
}

pub(crate) fn parse_delegation(value: Option<&str>) -> AppResult<Option<ParsedDelegation>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.contains(char::is_whitespace) {
        return Err(invalid_credential());
    }
    let secret = value
        .strip_prefix(DELEGATION_TOKEN_PREFIX)
        .ok_or_else(invalid_credential)?;
    validate_canonical_base64url(secret, SECRET_BYTES)?;
    Ok(Some(ParsedDelegation {
        token: value.to_owned(),
    }))
}

pub(crate) fn keyed_hash(key: &[u8], domain: &[u8], value: &[u8]) -> AppResult<Vec<u8>> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|_| AppError::Config("Pepper 长度无效".into()))?;
    mac.update(domain);
    mac.update(value);
    Ok(mac.finalize().into_bytes().to_vec())
}

pub(crate) fn invalid_credential() -> AppError {
    AppError::Authentication("Agent 凭据无效".into())
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

fn verify_mac(key: &[u8], domain: &[u8], value: &[u8], expected: &[u8]) -> AppResult<bool> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|_| AppError::Config("Pepper 长度无效".into()))?;
    mac.update(domain);
    mac.update(value);
    Ok(mac.verify_slice(expected).is_ok())
}

fn random_token(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    rand::rng().fill(&mut value[..]);
    URL_SAFE_NO_PAD.encode(value)
}
