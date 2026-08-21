use chrono::{DateTime, Utc};

use crate::PersistenceFuture;

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
pub trait AuthorizationMirrorTransaction: Send + Sync {
    fn increment_user_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> PersistenceFuture<'a, u64>;

    fn user_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<(i64, i32)>>;

    fn increment_tenant_epoch<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, i32>;

    fn increment_namespace_version<'a>(
        &'a self,
        tenant_id: &'a str,
        namespace: &'a str,
    ) -> PersistenceFuture<'a, i64>;

    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn record(&self, event: AuthorizationMirrorEvent) -> PersistenceFuture<'_, ()>;
}
