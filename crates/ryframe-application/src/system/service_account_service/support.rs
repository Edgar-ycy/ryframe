use super::*;

impl ServiceAccountService {
    pub(super) async fn bump_tenant_epoch(
        &self,
        transaction: &dyn AuthorizationMirrorTransaction,
        tenant_id: &str,
    ) -> AppResult<i32> {
        self.authorization_cache
            .increment_tenant_epoch_in_transaction(transaction, tenant_id)
            .await
    }

    /// 数据库事实与授权镜像修复 Outbox 已在同一事务提交；即时同步只用于降低传播延迟。
    /// 提交后 Redis 故障不能把已经生效的写操作伪装成失败，尤其不能丢失只显示一次的 Secret。
    pub(super) async fn sync_committed_authorization_state(
        &self,
        tenant_id: &str,
        authorization_epoch: i32,
        user_versions: &[(i64, i32)],
    ) {
        if !user_versions.is_empty()
            && let Err(error) = self
                .authorization_cache
                .sync_user_versions(tenant_id, user_versions)
                .await
        {
            tracing::warn!(
                tenant_id,
                %error,
                "服务账号写入已提交，用户授权镜像将由 Outbox 修复"
            );
        }
        if let Err(error) = self
            .authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await
        {
            tracing::warn!(
                tenant_id,
                %error,
                "服务账号写入已提交，租户授权镜像将由 Outbox 修复"
            );
        }
    }
}
