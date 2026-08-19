//! API 安全响应头中间件
//!
//! 自动为所有响应添加安全相关的 HTTP 头，提升应用安全性。
//!
//! 支持的安全头：
//! - `Content-Security-Policy` 内容安全策略（CSP）
//! - `Strict-Transport-Security` 严格传输安全策略（HSTS）
//! - `X-Content-Type-Options` 响应头
//! - `X-Frame-Options` 响应头
//! - `Referrer-Policy` 响应头
//! - `Permissions-Policy` 响应头
//! - `Cross-Origin-Opener-Policy` 响应头
//! - `Cross-Origin-Resource-Policy` 响应头

use std::collections::HashMap;

use axum::{
    http::{HeaderMap, HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

const DEFAULT_CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     base-uri 'self'; \
     font-src 'self' data:; \
     form-action 'self'; \
     frame-ancestors 'self'; \
     img-src 'self' data: blob:; \
     object-src 'none'; \
     script-src 'self'; \
     style-src 'self';";

const SWAGGER_UI_CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     base-uri 'self'; \
     connect-src 'self'; \
     font-src 'self' data:; \
     form-action 'self'; \
     frame-ancestors 'self'; \
     img-src 'self' data: blob:; \
     object-src 'none'; \
     script-src 'self'; \
     style-src 'self' 'unsafe-inline';";

const STRICT_CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     base-uri 'self'; \
     font-src 'self' data:; \
     form-action 'self'; \
     frame-ancestors 'none'; \
     img-src 'self' data: blob:; \
     object-src 'none'; \
     script-src 'self'; \
     style-src 'self'; \
     upgrade-insecure-requests";

/// 安全响应头配置
///
/// 所有字段均为 `Option`，为 `None` 时使用安全默认值。
#[derive(Clone, Debug)]
pub struct SecurityHeadersConfig {
    /// Content-Security-Policy 策略
    /// 默认只允许同源脚本、样式和资源。
    pub content_security_policy: Option<String>,

    /// Swagger UI 文档页面专用 Content-Security-Policy。
    ///
    /// Swagger UI 的请求示例和语法高亮组件会生成内联样式，因此只对文档页面允许
    /// `style-src 'unsafe-inline'`；脚本仍只能从同源加载。
    pub swagger_ui_content_security_policy: Option<String>,

    /// HSTS max-age（秒），仅 HTTPS 下生效
    /// 默认：`max-age=31536000; includeSubDomains`
    pub hsts_max_age: Option<u32>,

    /// 是否在 HSTS 中包含子域名
    pub hsts_include_subdomains: bool,

    /// X-Frame-Options 响应头。
    /// 默认：`DENY`
    pub x_frame_options: Option<String>,

    /// Referrer-Policy 响应头。
    /// 默认：`strict-origin-when-cross-origin`
    pub referrer_policy: Option<String>,

    /// Permissions-Policy（原 Feature-Policy）
    /// 默认：`camera=(), microphone=(), geolocation=()`
    pub permissions_policy: Option<String>,

    /// 自定义额外响应头
    pub custom_headers: HashMap<String, String>,
}

impl Default for SecurityHeadersConfig {
    fn default() -> Self {
        Self {
            content_security_policy: Some(DEFAULT_CONTENT_SECURITY_POLICY.to_string()),
            swagger_ui_content_security_policy: Some(
                SWAGGER_UI_CONTENT_SECURITY_POLICY.to_string(),
            ),
            hsts_max_age: Some(31536000),
            hsts_include_subdomains: true,
            x_frame_options: Some("DENY".to_string()),
            referrer_policy: Some("strict-origin-when-cross-origin".to_string()),
            permissions_policy: Some(
                "camera=(), microphone=(), geolocation=(), payment=()".to_string(),
            ),
            custom_headers: HashMap::new(),
        }
    }
}

impl SecurityHeadersConfig {
    /// 开发环境配置（禁用 HSTS，CSP 仍保持同源约束）
    pub fn development() -> Self {
        Self {
            hsts_max_age: None, // 开发环境禁用 HSTS
            x_frame_options: Some("SAMEORIGIN".to_string()),
            ..Default::default()
        }
    }

    /// 严格安全配置（适用于需要高安全性的生产环境）
    pub fn strict() -> Self {
        Self {
            content_security_policy: Some(STRICT_CONTENT_SECURITY_POLICY.to_string()),
            x_frame_options: Some("DENY".to_string()),
            permissions_policy: Some(
                "camera=(), microphone=(), geolocation=(), \
                 payment=(), usb=(), magnetometer=(), \
                 gyroscope=(), speaker=(), vibrate=()"
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}

struct HeaderWriter;

impl HeaderWriter {
    fn insert_static(headers: &mut HeaderMap, name: HeaderName, value: &'static str) {
        headers.insert(name, HeaderValue::from_static(value));
    }

    fn insert_optional(headers: &mut HeaderMap, name: HeaderName, value: Option<&str>) {
        if let Some(value) = value {
            Self::insert_value(headers, name, value);
        }
    }

    fn insert_value(headers: &mut HeaderMap, name: HeaderName, value: &str) {
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(name, value);
        }
    }
}

/// 安全响应头中间件
///
/// 为每个响应注入安全头。在 response 返回前修改 headers。
pub async fn security_headers_middleware(
    axum::extract::State(config): axum::extract::State<SecurityHeadersConfig>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let request_path = request.uri().path().to_owned();
    let mut response = next.run(request).await;

    // ========== X-Content-Type-Options 响应头 ==========
    HeaderWriter::insert_static(
        response.headers_mut(),
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff",
    );

    // ========== X-Frame-Options 响应头 ==========
    HeaderWriter::insert_optional(
        response.headers_mut(),
        header::X_FRAME_OPTIONS,
        config.x_frame_options.as_deref(),
    );

    // ========== X-DNS-Prefetch-Control 响应头 ==========
    HeaderWriter::insert_static(
        response.headers_mut(),
        HeaderName::from_static("x-dns-prefetch-control"),
        "off",
    );

    // ========== HSTS 传输安全策略 ==========
    if let Some(max_age) = config.hsts_max_age {
        let hsts = if config.hsts_include_subdomains {
            format!("max-age={}; includeSubDomains", max_age)
        } else {
            format!("max-age={}", max_age)
        };
        HeaderWriter::insert_value(
            response.headers_mut(),
            header::STRICT_TRANSPORT_SECURITY,
            &hsts,
        );
    }

    // ========== Content-Security-Policy 响应头 ==========
    let content_security_policy =
        if request_path.strip_prefix(crate::http::API_PREFIX) == Some("/swagger-ui") {
            config.swagger_ui_content_security_policy.as_deref()
        } else {
            config.content_security_policy.as_deref()
        };
    HeaderWriter::insert_optional(
        response.headers_mut(),
        header::CONTENT_SECURITY_POLICY,
        content_security_policy,
    );

    // ========== Referrer-Policy 响应头 ==========
    HeaderWriter::insert_optional(
        response.headers_mut(),
        header::REFERRER_POLICY,
        config.referrer_policy.as_deref(),
    );

    // ========== Permissions-Policy 响应头 ==========
    HeaderWriter::insert_optional(
        response.headers_mut(),
        HeaderName::from_static("permissions-policy"),
        config.permissions_policy.as_deref(),
    );

    // ========== Cross-Origin 跨源隔离响应头 ==========
    HeaderWriter::insert_static(
        response.headers_mut(),
        HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );

    HeaderWriter::insert_static(
        response.headers_mut(),
        HeaderName::from_static("cross-origin-resource-policy"),
        "same-origin",
    );

    // ========== 自定义头 ==========
    for (name, value) in &config.custom_headers {
        if let (Ok(h_name), Ok(h_value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            response.headers_mut().insert(h_name, h_value);
        }
    }

    response
}
