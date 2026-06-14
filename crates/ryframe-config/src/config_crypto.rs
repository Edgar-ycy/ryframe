//! 敏感配置加密存储
//!
//! 支持 AES-256-GCM 加密敏感配置值，格式为 `ENC[base64(ciphertext)]`。
//! 配置加载后自动解密所有标记字段。
//!
//! ## 用法
//!
//! 1. 设置环境变量 `CONFIG_MASTER_KEY` 为 32 字节的 Base64 编码密钥
//! 2. 在配置文件中将敏感值替换为 `ENC[...]` 格式
//! 3. 使用 `ConfigCrypto::encrypt(master_key, plaintext)` 生成加密值
//!
//! ```toml
//! [auth]
//! jwt_secret = "ENC[AQIDBAUG...]"
//! ```

use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce, aead::Aead};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rand::Rng;
use ryframe_common::{AppError, AppResult};

/// 加密值前缀标记
const ENCRYPTED_PREFIX: &str = "ENC[";
/// 加密值后缀标记
const ENCRYPTED_SUFFIX: &str = "]";

/// 配置加解密器
pub struct ConfigCrypto;

impl ConfigCrypto {
    /// 加密明文，返回 `ENC[base64(nonce + ciphertext)]`
    ///
    /// # 参数
    /// - `master_key`: 32 字节的密钥
    /// - `plaintext`: 要加密的明文
    #[allow(deprecated)]
    pub fn encrypt(master_key: &[u8], plaintext: &str) -> AppResult<String> {
        let key = Key::<Aes256Gcm>::from_slice(master_key);
        let cipher = Aes256Gcm::new(key);

        // 生成 12 字节随机 nonce
        let mut nonce_bytes = [0u8; 12];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // 加密：nonce + ciphertext
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| AppError::Config(format!("配置加密失败: {}", e)))?;

        // nonce(12) + ciphertext
        let mut combined = Vec::with_capacity(12 + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);

        Ok(format!(
            "{}{}{}",
            ENCRYPTED_PREFIX,
            BASE64.encode(&combined),
            ENCRYPTED_SUFFIX
        ))
    }

    /// 解密 `ENC[base64(...)]` 格式的值
    ///
    /// 如果值不以 `ENC[` 开头，则原样返回（明文模式）。
    #[allow(deprecated)]
    pub fn decrypt(master_key: &[u8], value: &str) -> AppResult<String> {
        // 非加密值原样返回
        if !value.starts_with(ENCRYPTED_PREFIX) || !value.ends_with(ENCRYPTED_SUFFIX) {
            return Ok(value.to_string());
        }

        let encoded = &value[ENCRYPTED_PREFIX.len()..value.len() - ENCRYPTED_SUFFIX.len()];

        let combined = BASE64
            .decode(encoded)
            .map_err(|e| AppError::Config(format!("配置解密 Base64 解码失败: {}", e)))?;

        if combined.len() < 13 {
            return Err(AppError::Config("配置解密数据长度不足".into()));
        }

        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        let key = Key::<Aes256Gcm>::from_slice(master_key);
        let cipher = Aes256Gcm::new(key);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| AppError::Config(format!("配置解密失败: 密钥不匹配或数据损坏 ({})", e)))?;

        String::from_utf8(plaintext)
            .map_err(|e| AppError::Config(format!("配置解密 UTF-8 解码失败: {}", e)))
    }

    /// 从环境变量 `CONFIG_MASTER_KEY` 获取主密钥
    ///
    /// 环境变量值应为 32 字节的 Base64 编码字符串。
    /// 如果未设置则返回 None（不启用加密解密）。
    pub fn load_master_key() -> Option<Vec<u8>> {
        let env_key = std::env::var("CONFIG_MASTER_KEY").ok()?;
        BASE64.decode(&env_key).ok().filter(|k| k.len() == 32)
    }

    /// 生成一个新的随机主密钥（Base64 编码的 32 字节密钥）
    pub fn generate_master_key() -> String {
        let mut key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        BASE64.encode(key)
    }
}

/// 解密配置中所有标记为 `ENC[...]` 的敏感字段
///
/// 如果 `CONFIG_MASTER_KEY` 未设置，则跳过解密（明文模式）。
pub fn decrypt_config(config: &mut crate::AppConfig) -> AppResult<()> {
    let master_key = match ConfigCrypto::load_master_key() {
        Some(k) => k,
        None => return Ok(()), // 未设置主密钥，跳过解密
    };

    // 解密 auth.jwt_secret
    config.auth.jwt_secret = ConfigCrypto::decrypt(&master_key, &config.auth.jwt_secret)?;

    // 解密 database 连接密码
    for conn in &mut config.database.connections {
        if !conn.password.is_empty() {
            conn.password = ConfigCrypto::decrypt(&master_key, &conn.password)?;
        }
    }

    // 解密 redis 密码
    if let Some(ref mut redis) = config.redis
        && !redis.password.is_empty()
    {
        redis.password = ConfigCrypto::decrypt(&master_key, &redis.password)?;
    }

    // 解密对象存储凭证
    if !config.object_storage.access_key.is_empty() {
        config.object_storage.access_key =
            ConfigCrypto::decrypt(&master_key, &config.object_storage.access_key)?;
    }
    if !config.object_storage.secret_key.is_empty() {
        config.object_storage.secret_key =
            ConfigCrypto::decrypt(&master_key, &config.object_storage.secret_key)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let master_key = ConfigCrypto::generate_master_key();
        let key_bytes = BASE64.decode(&master_key).unwrap();

        let plaintext = "my-super-secret-jwt-key-12345";
        let encrypted = ConfigCrypto::encrypt(&key_bytes, plaintext).unwrap();

        // 验证格式
        assert!(encrypted.starts_with("ENC["));
        assert!(encrypted.ends_with("]"));

        // 往返解密
        let decrypted = ConfigCrypto::decrypt(&key_bytes, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_plaintext_passthrough() {
        let key = vec![0u8; 32];
        let plaintext = "unencrypted-value";
        let result = ConfigCrypto::decrypt(&key, plaintext).unwrap();
        assert_eq!(result, plaintext);
    }

    #[test]
    fn test_wrong_key_fails() {
        let key1 = vec![1u8; 32];
        let key2 = vec![2u8; 32];

        let encrypted = ConfigCrypto::encrypt(&key1, "secret").unwrap();
        let result = ConfigCrypto::decrypt(&key2, &encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_master_key_is_valid() {
        let key = ConfigCrypto::generate_master_key();
        let decoded = BASE64.decode(&key).unwrap();
        assert_eq!(decoded.len(), 32);
    }
}
