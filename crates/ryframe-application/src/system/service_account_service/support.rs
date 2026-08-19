use super::*;

impl ServiceAccountService {
    pub(super) async fn lock_account(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
    ) -> AppResult<service_account::Model> {
        self.account_repo
            .find_by_id_in_txn(txn, tenant_id, account_id, ServiceAccountLock::Update)
            .await?
            .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))
    }

    pub(super) async fn bump_tenant_epoch(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<i32> {
        self.authorization_cache
            .increment_tenant_epoch_in_transaction(txn, tenant_id)
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

    pub(super) async fn user_permission_codes(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<HashSet<String>> {
        let roles = self
            .role_repo
            .find_user_roles(db, tenant_id, user_id)
            .await?;
        let ids = roles.into_iter().map(|role| role.id).collect::<Vec<_>>();
        Ok(self
            .permission_repo
            .find_role_perms(db, tenant_id, &ids)
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect())
    }

    pub(super) async fn account_permission_codes(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        account_id: i64,
    ) -> AppResult<HashSet<String>> {
        let role_ids = self
            .account_repo
            .role_ids(db, tenant_id, account_id)
            .await?;
        let enabled_role_ids = if role_ids.is_empty() {
            Vec::new()
        } else {
            role::Entity::find()
                .filter(role::Column::TenantId.eq(tenant_id))
                .filter(role::Column::Id.is_in(role_ids))
                .filter(role::Column::Status.eq(role::Model::STATUS_NORMAL))
                .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
                .all(db)
                .await
                .map_err(database_error)?
                .into_iter()
                .map(|role| role.id)
                .collect()
        };
        Ok(self
            .permission_repo
            .find_role_perms(db, tenant_id, &enabled_role_ids)
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect())
    }

    pub(super) async fn common_capability_keys_in_txn(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
        user_id: i64,
    ) -> AppResult<HashSet<String>> {
        let user_roles = self
            .role_repo
            .find_user_roles_all_status(txn, tenant_id, user_id)
            .await?
            .into_iter()
            .filter(|role| role.status == role::Model::STATUS_NORMAL)
            .map(|role| role.id)
            .collect::<Vec<_>>();
        let account_role_ids = self
            .account_repo
            .role_ids(txn, tenant_id, account_id)
            .await?;
        let account_roles = if account_role_ids.is_empty() {
            Vec::new()
        } else {
            role::Entity::find()
                .filter(role::Column::TenantId.eq(tenant_id))
                .filter(role::Column::Id.is_in(account_role_ids.iter().copied()))
                .filter(role::Column::Status.eq(role::Model::STATUS_NORMAL))
                .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
                .all(txn)
                .await
                .map_err(database_error)?
                .into_iter()
                .map(|role| role.id)
                .collect()
        };
        let user_permissions = permission_codes_in_txn(txn, tenant_id, &user_roles).await?;
        let account_permissions = permission_codes_in_txn(txn, tenant_id, &account_roles).await?;
        Ok(
            common_capabilities(&self.capabilities, &user_permissions, &account_permissions)
                .into_iter()
                .map(|capability| capability.key)
                .collect(),
        )
    }

    pub(super) async fn delegation_vo<C>(
        &self,
        db: &C,
        delegation: service_delegation::Model,
    ) -> AppResult<ServiceDelegationVo>
    where
        C: sea_orm::ConnectionTrait,
    {
        let keys = self
            .delegation_repo
            .capability_keys(db, &delegation.tenant_id, delegation.id)
            .await?;
        Ok(delegation_vo_with_keys(delegation, keys))
    }

    pub(super) async fn delegations_with_capabilities(
        &self,
        db: &DatabaseConnection,
        rows: Vec<service_delegation::Model>,
    ) -> AppResult<Vec<ServiceDelegationVo>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let delegation_ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
        let capabilities = service_delegation_capability::Entity::find()
            .filter(service_delegation_capability::Column::TenantId.eq(&rows[0].tenant_id))
            .filter(
                service_delegation_capability::Column::DelegationId
                    .is_in(delegation_ids.iter().copied()),
            )
            .order_by_asc(service_delegation_capability::Column::DelegationId)
            .order_by_asc(service_delegation_capability::Column::CapabilityKey)
            .all(db)
            .await
            .map_err(database_error)?;
        let mut by_delegation: HashMap<i64, Vec<String>> = HashMap::new();
        for capability in capabilities {
            by_delegation
                .entry(capability.delegation_id)
                .or_default()
                .push(capability.capability_key);
        }
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let keys = by_delegation.remove(&row.id).unwrap_or_default();
            result.push(delegation_vo_with_keys(row, keys));
        }
        Ok(result)
    }
}
