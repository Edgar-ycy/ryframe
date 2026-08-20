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
        let transaction = self.write.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            let mut account = transaction
                .lock_account(tenant_id, account_id)
                .await?
                .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
            transaction
                .replace_roles(tenant_id, account_id, &role_ids)
                .await?;
            account.authorization_version = account.authorization_version.saturating_add(1);
            account.updated_at = transaction.authorization_mirror().database_now().await?;
            transaction.save_account(tenant_id, account).await?;
            self.bump_tenant_epoch(transaction.authorization_mirror(), tenant_id)
                .await
        }
        .await;
        match result {
            Ok(epoch) => {
                transaction.commit().await?;
                self.sync_committed_authorization_state(tenant_id, epoch, &[])
                    .await;
                Ok(())
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }
}
