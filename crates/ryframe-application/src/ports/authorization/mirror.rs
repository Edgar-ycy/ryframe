use chrono::{DateTime, Utc};

/// 写入授权镜像 Outbox 的应用事件。
#[derive(Debug)]
pub struct AuthorizationMirrorEvent {
    pub tenant_id: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub payload: serde_json::Value,
    pub available_at: DateTime<Utc>,
    pub max_attempts: i32,
    pub dedupe_key: String,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

/// 业务写事务提供的授权版本与镜像事件原子持久化能力。
#[async_trait::async_trait]
pub trait AuthorizationMirrorTransaction: Send + Sync {
    async fn increment_user_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<u64>;

    async fn user_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<(i64, i32)>>;

    async fn increment_tenant_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<i32>;

    async fn increment_namespace_version<'a>(
        &'a self,
        tenant_id: &'a str,
        namespace: &'a str,
    ) -> ryframe_kernel::AppResult<i64>;

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn record(&self, event: AuthorizationMirrorEvent) -> ryframe_kernel::AppResult<()>;
}
