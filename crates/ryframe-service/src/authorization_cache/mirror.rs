use ryframe_db::{
    CacheNamespaceVersionRepository, OutboxEventRepository, RecordOutboxEvent, TenantRepository,
    UserRepository, validate_cache_namespace,
};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::DatabaseTransaction;

use super::*;

impl AuthorizationCache {
    /// 在业务事务内递增一组用户授权版本，并原子记录 Redis 镜像修复事件。
    pub async fn increment_user_versions_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        user_ids: &[i64],
    ) -> AppResult<Vec<(i64, i32)>> {
        let mut user_ids = user_ids.to_vec();
        user_ids.sort_unstable();
        user_ids.dedup();
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }

        let affected = UserRepository
            .increment_authorization_versions(transaction, tenant_id, &user_ids)
            .await?;
        if affected != user_ids.len() as u64 {
            return Err(AppError::NotFound("用户不存在".into()));
        }
        let versions = UserRepository
            .find_authorization_versions(transaction, tenant_id, &user_ids)
            .await?;
        if versions.len() != user_ids.len() {
            return Err(AppError::NotFound("用户不存在".into()));
        }
        self.record_user_mirror_updates_in_transaction(transaction, tenant_id, &versions)
            .await?;
        Ok(versions)
    }

    /// 为已由调用方递增的单个用户版本记录镜像修复事件。
    pub async fn record_user_mirror_update_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        user_id: i64,
        authorization_version: i32,
    ) -> AppResult<()> {
        self.record_user_mirror_updates_in_transaction(
            transaction,
            tenant_id,
            &[(user_id, authorization_version)],
        )
        .await
    }

    /// 在业务事务内递增租户授权纪元，并原子记录 Redis 镜像修复事件。
    pub async fn increment_tenant_epoch_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<i32> {
        let authorization_epoch = TenantRepository
            .increment_authorization_epoch_in_txn(transaction, tenant_id)
            .await?;
        if self.is_enabled() {
            let now = OutboxEventRepository.database_utc_now(transaction).await?;
            let payload = AuthorizationMirrorUpdate::Tenant {
                tenant_id: tenant_id.to_owned(),
                authorization_epoch,
            };
            OutboxEventRepository
                .record_in_transaction(
                    transaction,
                    mirror_event(
                        tenant_id,
                        "tenant",
                        tenant_id,
                        i64::from(authorization_epoch),
                        payload,
                        now,
                    )?,
                    now,
                )
                .await?;
        }
        Ok(authorization_epoch)
    }

    /// 在业务事务中递增数据库权威命名空间版本，并原子写入 Outbox。
    ///
    /// 即使 Redis 未启用，数据库计数器仍会推进；以后重新启用 Redis 时可以从数据库
    /// 恢复权威版本，而不需要猜测或扫描旧缓存键。
    pub async fn record_namespace_version_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        namespace: &str,
    ) -> AppResult<i64> {
        validate_cache_namespace(namespace)?;
        let namespace_version = CacheNamespaceVersionRepository
            .increment_in_transaction(transaction, tenant_id, namespace)
            .await?;
        let now = OutboxEventRepository.database_utc_now(transaction).await?;
        let payload = AuthorizationMirrorUpdate::TenantCacheNamespace {
            tenant_id: tenant_id.to_owned(),
            namespace: namespace.to_owned(),
            namespace_version,
        };
        OutboxEventRepository
            .record_in_transaction(
                transaction,
                mirror_event(
                    tenant_id,
                    "tenant_cache_namespace",
                    namespace,
                    namespace_version,
                    payload,
                    now,
                )?,
                now,
            )
            .await?;
        Ok(namespace_version)
    }

    async fn record_user_mirror_updates_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        versions: &[(i64, i32)],
    ) -> AppResult<()> {
        if !self.is_enabled() || versions.is_empty() {
            return Ok(());
        }
        let now = OutboxEventRepository.database_utc_now(transaction).await?;
        for (user_id, authorization_version) in versions {
            let aggregate_id = user_id.to_string();
            let payload = AuthorizationMirrorUpdate::User {
                tenant_id: tenant_id.to_owned(),
                user_id: *user_id,
                authorization_version: *authorization_version,
            };
            OutboxEventRepository
                .record_in_transaction(
                    transaction,
                    mirror_event(
                        tenant_id,
                        "user",
                        &aggregate_id,
                        i64::from(*authorization_version),
                        payload,
                        now,
                    )?,
                    now,
                )
                .await?;
        }
        Ok(())
    }
}

fn mirror_event(
    tenant_id: &str,
    aggregate_type: &str,
    aggregate_id: &str,
    version: i64,
    payload: AuthorizationMirrorUpdate,
    available_at: chrono::DateTime<chrono::Utc>,
) -> AppResult<RecordOutboxEvent> {
    let trace_context = crate::trace_context::current_trace_context();
    Ok(RecordOutboxEvent {
        tenant_id: Some(tenant_id.to_owned()),
        event_type: AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE.to_owned(),
        aggregate_type: aggregate_type.to_owned(),
        aggregate_id: aggregate_id.to_owned(),
        payload: serde_json::to_value(payload)
            .map_err(|error| AppError::Internal(format!("序列化授权镜像事件失败: {error}")))?,
        available_at,
        max_attempts: 20,
        dedupe_key: Some(format!(
            "authorization-mirror:{tenant_id}:{aggregate_type}:{aggregate_id}:{version}"
        )),
        traceparent: trace_context.traceparent,
        tracestate: trace_context.tracestate,
    })
}
