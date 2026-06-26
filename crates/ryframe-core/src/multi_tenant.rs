//! 多租户支持
//!
//! 提供租户识别、上下文传递和数据隔离能力：
//! - **租户识别**：从请求 Header / Subdomain 提取 tenant_id
//! - **租户上下文**：通过 `axum::Extension<TenantContext>` 在请求生命周期中传递
//! - **数据隔离**：共享表 + tenant_id 列过滤策略
//!
//! # 隔离策略
//!
//! | 策略 | 说明 | 适用场景 |
//! |------|------|----------|
//! | `SharedTable` | 共享表 + tenant_id 列过滤 | 中小规模 SaaS |
//! | `DatabasePerTenant` | 独立数据库 | 高隔离要求 |
//!
//! # 使用示例
//!
//! ```
//! use ryframe_core::multi_tenant::{TenantConfig, TenantContext, TenantFilter};
//! use ryframe_core::multi_tenant::{ExtractionMethod, IsolationStrategy, TenantQuota, TenantIsolation};
//!
//! // 配置租户识别方式
//! let config = TenantConfig {
//!     extraction_method: ExtractionMethod::Header("X-Tenant-Id".into()),
//!     isolation_strategy: IsolationStrategy::SharedTable,
//!     default_tenant: None,
//! };
//! assert!(matches!(config.extraction_method, ExtractionMethod::Header(_)));
//!
//! // 创建租户上下文
//! let ctx = TenantContext::admin();
//! assert!(ctx.is_admin);
//!
//! // 创建租户过滤器
//! let filter = TenantFilter::new("inner_repo").with_context(&ctx);
//! assert!(filter.is_admin());
//!
//! // 租户配额
//! let quota = TenantQuota::default();
//! assert_eq!(quota.max_users, 100);
//! ```

use axum::{
    extract::State,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use std::{
    sync::Arc,
    sync::OnceLock,
    time::{Duration, Instant},
};

tokio::task_local! {
    /// Request-scoped tenant identity. Repository code reads this instead of a
    /// process-wide default so concurrent requests cannot leak tenant scope.
    static REQUEST_TENANT_CONTEXT: TenantContext;
}

#[cfg(debug_assertions)]
static DEBUG_TENANT_FALLBACK: OnceLock<String> = OnceLock::new();

// ============ 核心类型 ============

/// 租户上下文（注入到 request extensions）
#[derive(Debug, Clone)]
pub struct TenantContext {
    /// 当前租户 ID
    pub tenant_id: String,
    /// 是否系统管理员（可跨租户操作）
    pub is_admin: bool,
}

impl TenantContext {
    /// 从 request extensions 提取租户上下文
    pub fn from_request(req: &axum::http::Request<axum::body::Body>) -> Option<&Self> {
        req.extensions().get::<Self>()
    }

    /// 创建系统管理员上下文（无租户限制）
    pub fn admin() -> Self {
        Self {
            tenant_id: "system".into(),
            is_admin: true,
        }
    }
}

/// Returns the request tenant for repository and service code.
///
/// Code that runs outside an HTTP request must wrap its work in
/// `with_tenant_context`; otherwise it receives a sentinel tenant that cannot
/// accidentally match or write system-owned data.
pub fn current_tenant_id() -> String {
    REQUEST_TENANT_CONTEXT
        .try_with(|context| context.tenant_id.clone())
        .unwrap_or_else(|_| {
            #[cfg(debug_assertions)]
            if let Some(tenant_id) = DEBUG_TENANT_FALLBACK.get() {
                return tenant_id.clone();
            }
            tracing::error!("missing tenant context; refusing to fall back to system tenant");
            "__missing_tenant_context__".to_string()
        })
}

#[cfg(debug_assertions)]
pub fn set_debug_tenant_fallback(tenant_id: impl Into<String>) {
    let _ = DEBUG_TENANT_FALLBACK.set(tenant_id.into());
}

/// Runs a future with an explicit tenant scope. Middleware uses this to make
/// the authenticated tenant available throughout asynchronous repository calls.
pub async fn with_tenant_context<F>(context: TenantContext, future: F) -> F::Output
where
    F: std::future::Future,
{
    REQUEST_TENANT_CONTEXT.scope(context, future).await
}

#[derive(Clone, Debug)]
pub struct TenantRateLimitCache {
    ttl: Duration,
    entries: Arc<DashMap<String, TenantRateLimitEntry>>,
}

#[derive(Clone, Debug)]
struct TenantRateLimitEntry {
    limit_per_minute: u32,
    expires_at: Instant,
}

impl Default for TenantRateLimitCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(30))
    }
}

impl TenantRateLimitCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Arc::new(DashMap::new()),
        }
    }

    pub fn get(&self, tenant_id: &str) -> Option<u32> {
        let now = Instant::now();
        let cached = self.entries.get(tenant_id)?;
        if cached.expires_at > now {
            Some(cached.limit_per_minute)
        } else {
            drop(cached);
            self.entries.remove(tenant_id);
            None
        }
    }

    pub fn insert(&self, tenant_id: impl Into<String>, limit_per_minute: u32) {
        self.entries.insert(
            tenant_id.into(),
            TenantRateLimitEntry {
                limit_per_minute,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    pub fn invalidate(&self, tenant_id: &str) {
        self.entries.remove(tenant_id);
    }
}

/// 租户提取方式
#[derive(Debug, Clone)]
pub enum ExtractionMethod {
    /// 从 HTTP Header 提取（如 X-Tenant-Id）
    Header(String),
    /// 从子域名提取（如 tenant1.example.com → tenant1）
    Subdomain,
}

/// 数据隔离策略
#[derive(Debug, Clone, PartialEq)]
pub enum IsolationStrategy {
    /// 共享表 + tenant_id 列过滤
    SharedTable,
    /// 独立数据库（需数据源路由支持）
    DatabasePerTenant,
}

/// 租户配置
#[derive(Debug, Clone)]
pub struct TenantConfig {
    /// 租户 ID 提取方式
    pub extraction_method: ExtractionMethod,
    /// 数据隔离策略
    pub isolation_strategy: IsolationStrategy,
    /// 默认租户 ID（无租户信息时使用，None 表示拒绝请求）
    pub default_tenant: Option<String>,
}

impl Default for TenantConfig {
    fn default() -> Self {
        Self {
            extraction_method: ExtractionMethod::Header("X-Tenant-Id".into()),
            isolation_strategy: IsolationStrategy::SharedTable,
            default_tenant: None,
        }
    }
}

// ============ 租户识别中间件 ============

/// 租户识别中间件
///
/// 从请求中提取 tenant_id，构造 `TenantContext` 并注入 `request.extensions()`。
/// 如果未找到租户信息且未配置默认租户，返回 403 Forbidden。
pub async fn tenant_middleware(
    State(config): State<Arc<TenantConfig>>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if matches!(path, "/" | "/health")
        || path.starts_with("/api/v1/api-docs/")
        || path.starts_with("/api/v1/swagger-ui")
        || path == "/api/v1/version"
    {
        return next.run(request).await;
    }

    let tenant_id = extract_tenant_id(&request, &config);

    match tenant_id {
        Some(id) => {
            let context = TenantContext {
                tenant_id: id,
                is_admin: false,
            };
            request.extensions_mut().insert(context.clone());
            with_tenant_context(context, next.run(request)).await
        }
        None => {
            // 无租户信息 → 403
            (
                StatusCode::FORBIDDEN,
                "{\"code\":403,\"message\":\"缺少租户信息\"}",
            )
                .into_response()
        }
    }
}

/// 从请求中提取 tenant_id
fn extract_tenant_id(request: &axum::extract::Request, config: &TenantConfig) -> Option<String> {
    match &config.extraction_method {
        ExtractionMethod::Header(header_name) => request
            .headers()
            .get(header_name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),

        ExtractionMethod::Subdomain => {
            let host = request
                .headers()
                .get(axum::http::header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            // 提取第一个 "." 之前的部分作为租户 ID
            host.split('.').next().map(|s| s.to_string())
        }
    }
    .or_else(|| config.default_tenant.clone())
}

// ============ 数据隔离辅助 ============

/// 租户数据隔离 trait
///
/// 仓库层实现此 trait 以自动过滤租户数据。
/// SharedTable 策略下，所有查询自动添加 `tenant_id = ?` 条件。
#[async_trait::async_trait]
pub trait TenantIsolation {
    /// 设置当前租户上下文
    fn set_tenant(&mut self, tenant_id: &str);

    /// 获取当前租户 ID
    fn tenant_id(&self) -> Option<&str>;

    /// 是否为管理员模式（可跨租户）
    fn is_admin(&self) -> bool;
}

/// 共享表租户过滤实现
///
/// 包装仓库，在查询前自动注入 tenant_id 过滤条件。
#[derive(Debug, Clone)]
pub struct TenantFilter<T> {
    inner: T,
    tenant_id: Option<String>,
    is_admin: bool,
}

impl<T> TenantFilter<T> {
    /// 创建新的租户过滤器
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            tenant_id: None,
            is_admin: false,
        }
    }

    /// 从 TenantContext 设置租户
    pub fn with_context(mut self, ctx: &TenantContext) -> Self {
        self.tenant_id = Some(ctx.tenant_id.clone());
        self.is_admin = ctx.is_admin;
        self
    }

    /// 获取内部实例
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// 获取可变内部实例
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// 获取租户过滤条件
    pub fn tenant_filter(&self) -> Option<&str> {
        if self.is_admin {
            None // 管理员不过滤
        } else {
            self.tenant_id.as_deref()
        }
    }
}

impl<T> TenantIsolation for TenantFilter<T> {
    fn set_tenant(&mut self, tenant_id: &str) {
        self.tenant_id = Some(tenant_id.to_string());
    }

    fn tenant_id(&self) -> Option<&str> {
        self.tenant_id.as_deref()
    }

    fn is_admin(&self) -> bool {
        self.is_admin
    }
}

// ============ 租户配额管理 ============

/// 租户配额
#[derive(Debug, Clone)]
pub struct TenantQuota {
    /// 最大用户数
    pub max_users: u32,
    /// 最大角色数
    pub max_roles: u32,
    /// 最大存储容量 (MB)
    pub max_storage_mb: u64,
    /// 最大 API 请求数/分钟
    pub max_requests_per_min: u32,
}

impl Default for TenantQuota {
    fn default() -> Self {
        Self {
            max_users: 100,
            max_roles: 20,
            max_storage_mb: 1024,
            max_requests_per_min: 1000,
        }
    }
}

/// 租户配额检查结果
#[derive(Debug)]
pub enum QuotaCheck {
    /// 配额充足
    Ok,
    /// 超出配额
    Exceeded {
        resource: String,
        current: u64,
        limit: u64,
    },
}

impl TenantQuota {
    /// 检查用户数配额
    pub fn check_users(&self, current_count: u32) -> QuotaCheck {
        if current_count >= self.max_users {
            QuotaCheck::Exceeded {
                resource: "users".into(),
                current: current_count as u64,
                limit: self.max_users as u64,
            }
        } else {
            QuotaCheck::Ok
        }
    }

    /// 检查存储配额
    pub fn check_storage(&self, current_mb: u64) -> QuotaCheck {
        if current_mb >= self.max_storage_mb {
            QuotaCheck::Exceeded {
                resource: "storage".into(),
                current: current_mb,
                limit: self.max_storage_mb,
            }
        } else {
            QuotaCheck::Ok
        }
    }
}

// ============ 测试 ============

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_context_admin() {
        let ctx = TenantContext::admin();
        assert!(ctx.is_admin);
        assert_eq!(ctx.tenant_id, "system");
    }

    #[test]
    fn test_tenant_filter_admin_no_filter() {
        let ctx = TenantContext::admin();
        let filter = TenantFilter::new("inner").with_context(&ctx);
        assert!(filter.tenant_filter().is_none()); // 管理员不过滤
        assert!(filter.is_admin());
    }

    #[test]
    fn test_tenant_filter_user_with_filter() {
        let ctx = TenantContext {
            tenant_id: "tenant-001".into(),
            is_admin: false,
        };
        let filter = TenantFilter::new("inner").with_context(&ctx);
        assert_eq!(filter.tenant_filter(), Some("tenant-001"));
        assert!(!filter.is_admin());
    }

    #[test]
    fn test_quota_users_ok() {
        let quota = TenantQuota::default();
        assert!(matches!(quota.check_users(50), QuotaCheck::Ok));
    }

    #[test]
    fn test_quota_users_exceeded() {
        let quota = TenantQuota::default();
        assert!(matches!(
            quota.check_users(100),
            QuotaCheck::Exceeded { .. }
        ));
    }

    #[test]
    fn test_quota_storage_exceeded() {
        let quota = TenantQuota::default();
        assert!(matches!(
            quota.check_storage(2048),
            QuotaCheck::Exceeded { .. }
        ));
    }

    #[test]
    fn test_default_config() {
        let config = TenantConfig::default();
        assert!(matches!(
            config.extraction_method,
            ExtractionMethod::Header(_)
        ));
        assert_eq!(config.isolation_strategy, IsolationStrategy::SharedTable);
    }
}
