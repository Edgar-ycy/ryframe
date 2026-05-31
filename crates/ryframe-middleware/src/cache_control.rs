//! HTTP 缓存头中间件
//!
//! 提供 ETag 和 Cache-Control 响应头支持：
//! - 自动为 GET/HEAD 请求生成 ETag（响应体 SHA-256 哈希）
//! - 检查 If-None-Match → 返回 304 Not Modified
//! - 按路由模式注入 Cache-Control / Expires 头
//! - 避免重复传输不变的资源，降低带宽消耗
//!
//! # 使用示例
//!
//! ```ignore
//! use ryframe_middleware::cache_control::{CacheControlConfig, cache_control_middleware};
//!
//! let config = CacheControlConfig {
//!     default_max_age: 3600,          // 默认缓存 1 小时
//!     enable_etag: true,              // 启用 ETag 生成
//!     custom_rules: vec![
//!         ("/api/v1/public/*".into(), "public, max-age=86400".into()),
//!         ("/api/v1/config/*".into(), "no-cache".into()),
//!     ],
//! };
//!
//! Router::new()
//!     .layer(from_fn_with_state(config, cache_control_middleware))
//! ```

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};

// ============ 配置 ============

/// 缓存控制配置
#[derive(Debug, Clone)]
pub struct CacheControlConfig {
    /// 默认 max-age（秒），0 表示不缓存
    pub default_max_age: u64,
    /// 是否生成 ETag
    pub enable_etag: bool,
    /// 自定义规则：按路径前缀匹配 (prefix, cache_control_value)
    /// 例如：("/api/v1/static/", "public, max-age=86400, immutable")
    pub custom_rules: Vec<(String, String)>,
}

impl Default for CacheControlConfig {
    fn default() -> Self {
        Self {
            default_max_age: 0,
            enable_etag: true,
            custom_rules: vec![],
        }
    }
}

impl CacheControlConfig {
    /// 创建默认不缓存的配置
    pub fn no_cache() -> Self {
        Self::default()
    }

    /// 创建带默认 max-age 的配置
    pub fn with_max_age(max_age_secs: u64) -> Self {
        Self {
            default_max_age: max_age_secs,
            enable_etag: true,
            custom_rules: vec![],
        }
    }

    /// 添加自定义规则
    pub fn with_rule(mut self, prefix: &str, cache_control: &str) -> Self {
        self.custom_rules
            .push((prefix.into(), cache_control.into()));
        self
    }

    /// 为静态资源预设规则
    pub fn for_static_assets() -> Self {
        Self {
            default_max_age: 0,
            enable_etag: true,
            custom_rules: vec![
                (
                    "/static/".into(),
                    "public, max-age=31536000, immutable".into(),
                ),
                (
                    "/assets/".into(),
                    "public, max-age=31536000, immutable".into(),
                ),
            ],
        }
    }

    /// 根据请求路径匹配 Cache-Control 值
    fn cache_control_for(&self, path: &str) -> Option<&str> {
        for (prefix, value) in &self.custom_rules {
            if path.starts_with(prefix.as_str()) {
                return Some(value.as_str());
            }
        }
        if self.default_max_age > 0 {
            return None; // 使用下游默认值
        }
        None
    }
}

// ============ ETag 工具函数 ============

/// 计算字节序列的 ETag（SHA-256 前 8 字节的 hex 表示）
fn compute_etag(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    // 取前 8 字节，格式化为 16 位十六进制字符串
    let hex: String = result[..8].iter().map(|b| format!("{:02x}", b)).collect();
    format!("\"{}\"", hex)
}

/// 基于 SystemTime 计算弱 ETag
#[allow(dead_code)]
fn compute_weak_etag(timestamp: u64) -> String {
    format!("W/\"{:x}\"", timestamp)
}

/// 从 HeaderMap 提取 If-None-Match 值
fn get_if_none_match(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
}

// ============ 中间件 ============

/// 缓存控制中间件
///
/// 流程：
/// 1. 仅对 GET/HEAD 请求生效
/// 2. 计算响应体 ETag
/// 3. 检查 If-None-Match → 匹配则返回 304
/// 4. 注入 Cache-Control / ETag 响应头
pub async fn cache_control_middleware(
    State(config): State<Arc<CacheControlConfig>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    // 仅对安全方法生效
    if method != Method::GET && method != Method::HEAD {
        return next.run(request).await;
    }

    let if_none_match = get_if_none_match(request.headers());

    // 执行下游 Handler
    let response = next.run(request).await;

    // 非 2xx 响应不缓存
    if !response.status().is_success() {
        return response;
    }

    // 获取响应体和 headers
    let (mut parts, body) = response.into_parts();
    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(_) => {
            return Response::from_parts(parts, Body::from("Failed to read response body"));
        }
    };

    let body_vec = body_bytes.to_vec();
    let etag = compute_etag(&body_vec);

    // 检查 If-None-Match
    if let Some(ref client_etag) = if_none_match
        && client_etag == &etag
    {
        // 304 Not Modified
        let mut headers = parts.headers.clone();
        headers.insert(
            header::ETAG,
            HeaderValue::from_str(&etag).unwrap_or(HeaderValue::from_static("\"\"")),
        );
        // 304 响应不带 body
        return (StatusCode::NOT_MODIFIED, headers, Body::empty()).into_response();
    }

    // 注入 Cache-Control
    if let Some(cc_value) = config.cache_control_for(&path) {
        parts.headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_str(cc_value).unwrap_or(HeaderValue::from_static("no-cache")),
        );
    } else if config.default_max_age > 0 {
        let cc = format!("private, max-age={}", config.default_max_age);
        parts.headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_str(&cc).unwrap_or(HeaderValue::from_static("no-cache")),
        );
    }

    // 注入 ETag
    if config.enable_etag && !method.eq(&Method::HEAD) {
        parts.headers.insert(
            header::ETAG,
            HeaderValue::from_str(&etag).unwrap_or(HeaderValue::from_static("\"\"")),
        );
    }

    Response::from_parts(parts, Body::from(body_vec))
}

// ============ AST (Adaptive Stale Time) 辅助 ============

/// 计算 Expires 头值（当前时间 + ttl）
pub fn compute_expires(ttl: Duration) -> HeaderValue {
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + ttl.as_secs();
    // 使用简化的日期格式（完整 HTTP-date 可用 chrono/时间库）
    HeaderValue::from_str(&format!("{}", expires)).unwrap_or(HeaderValue::from_static("0"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_etag_same_content() {
        let etag1 = compute_etag(b"hello world");
        let etag2 = compute_etag(b"hello world");
        assert_eq!(etag1, etag2);
    }

    #[test]
    fn test_compute_etag_different_content() {
        let etag1 = compute_etag(b"hello world");
        let etag2 = compute_etag(b"hello WORLD");
        assert_ne!(etag1, etag2);
    }

    #[test]
    fn test_cache_control_rules() {
        let config = CacheControlConfig::default()
            .with_rule("/api/static/", "public, max-age=3600")
            .with_rule("/api/dynamic/", "no-cache");

        assert_eq!(
            config.cache_control_for("/api/static/image.png"),
            Some("public, max-age=3600")
        );
        assert_eq!(
            config.cache_control_for("/api/dynamic/user"),
            Some("no-cache")
        );
        assert_eq!(config.cache_control_for("/api/other"), None);
    }

    #[test]
    fn test_default_config() {
        let config = CacheControlConfig::default();
        assert_eq!(config.default_max_age, 0);
        assert!(config.enable_etag);
        assert!(config.custom_rules.is_empty());
    }

    #[test]
    fn test_weak_etag() {
        let etag = compute_weak_etag(1717200000);
        assert!(etag.starts_with("W/\""));
    }
}
