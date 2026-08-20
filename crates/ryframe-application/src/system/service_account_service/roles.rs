use super::*;

impl ServiceAccountService {
    pub async fn account_role_ids(
        &self,
        actor: &ActorContext,
        account_id: i64,
    ) -> AppResult<Vec<String>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let role_ids = self
            .read
            .enabled_account_role_ids(tenant_id, account_id)
            .await?
            .ok_or_else(|| AppError::NotFound("可用的服务账号不存在".into()))?;
        Ok(role_ids.into_iter().map(|id| id.to_string()).collect())
    }

    pub async fn replace_account_roles(
        &self,
        actor: &ActorContext,
        account_id: i64,
        mut role_ids: Vec<i64>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        role_ids.sort_unstable();
        role_ids.dedup();
        let txn = self.db.write().begin().await.map_err(database_error)?;
        self.account_repo
            .lock_tenant_in_txn(&txn, tenant_id, ServiceAccountLock::Update)
            .await?;
        self.lock_account(&txn, tenant_id, account_id).await?;
        self.account_repo
            .replace_roles_in_txn(&txn, tenant_id, account_id, &role_ids)
            .await?;
        self.account_repo
            .increment_authorization_version_in_txn(&txn, tenant_id, account_id)
            .await?;
        let epoch = self.bump_tenant_epoch(&txn, tenant_id).await?;
        crate::commit_current_audit(txn).await?;
        self.sync_committed_authorization_state(tenant_id, epoch, &[])
            .await;
        Ok(())
    }
}
