#![allow(dead_code)]

use base64::{Engine, engine::general_purpose::STANDARD};
use uuid::Uuid;

/// MD5 哈希（返回 32 位小写十六进制字符串）
pub fn md5_hex(input: &str) -> String {
    format!("{:x}", md5::compute(input.as_bytes()))
}

/// Base64 编码
pub fn base64_encode(input: &str) -> String {
    STANDARD.encode(input.as_bytes())
}

/// Base64 解码，失败返回 None
pub fn base64_decode(input: &str) -> Option<String> {
    STANDARD
        .decode(input)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// 生成 UUID v4（无连字符的 32 位字符串）
pub fn uuid_v4_simple() -> String {
    Uuid::new_v4().simple().to_string()
}

/// 生成 UUID v4（标准格式，带连字符）
pub fn uuid_v4() -> String {
    Uuid::new_v4().to_string()
}
