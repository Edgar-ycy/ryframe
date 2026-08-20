use super::*;

impl ServiceAccountService {
    pub async fn common_delegated_capabilities(
        &self,
        actor: &ActorContext,
        account_id: i64,
    ) -> AppResult<Vec<ServiceCapabilityDescriptor>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        self.account_repo
            .find_by_id(&db, tenant_id, account_id)
            .await?
            .filter(service_account::Model::is_enabled)
            .ok_or_else(|| AppError::NotFound("可用的服务账号不存在".into()))?;
        let user_permissions = self
            .user_permission_codes(&db, tenant_id, actor.user_id)
            .await?;
        let account_permissions = self
            .account_permission_codes(&db, tenant_id, account_id)
            .await?;
        Ok(common_capabilities(
            &self.capabilities,
            &user_permissions,
            &account_permissions,
        ))
    }

    /// 个人委托页可见的候选账号，不要求服务账号管理权限。
    pub async fn delegation_targets(
        &self,
        actor: &ActorContext,
    ) -> AppResult<Vec<ServiceDelegationTargetVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let user_permissions = self
            .user_permission_codes(&db, tenant_id, actor.user_id)
            .await?;
        const MAX_DELEGATION_TARGETS: u64 = 1_000;
        let account_query = service_account::Entity::find()
            .filter(service_account::Column::TenantId.eq(tenant_id))
            .filter(service_account::Column::Status.eq(service_account::Model::STATUS_NORMAL))
            .filter(service_account::Column::DelFlag.eq(service_account::Model::DEL_FLAG_NORMAL));
        if account_query
            .clone()
            .count(&db)
            .await
            .map_err(database_error)?
            > MAX_DELEGATION_TARGETS
        {
            return Err(AppError::Validation(
                "当前租户可用服务账号超过 1000 个，请由管理员收敛账号数量".into(),
            ));
        }
        let accounts = account_query
            .order_by_asc(service_account::Column::Code)
            .all(&db)
            .await
            .map_err(database_error)?;
        let account_ids = accounts
            .iter()
            .map(|account| account.id)
            .collect::<Vec<_>>();
        let account_permissions =
            account_permission_codes_for_accounts(&db, tenant_id, &account_ids).await?;
        let mut targets = Vec::new();
        for account in accounts {
            let permissions = account_permissions
                .get(&account.id)
                .cloned()
                .unwrap_or_default();
            let capabilities =
                common_capabilities(&self.capabilities, &user_permissions, &permissions);
            if !capabilities.is_empty() {
                targets.push(ServiceDelegationTargetVo {
                    account_id: account.id.to_string(),
                    code: account.code,
                    name: account.name,
                    capabilities,
                });
            }
        }
        Ok(targets)
    }

    pub async fn create_delegation(
        &self,
        actor: &ActorContext,
        command: CreateDelegationCommand,
    ) -> AppResult<CreatedDelegationVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let reason = required_text(command.reason, "委托原因", 500)?;
        let idempotency_key = validate_idempotency_key(command.idempotency_key)?;
        let mut capability_keys = command.capability_keys;
        capability_keys.sort();
        capability_keys.dedup();
        if capability_keys.is_empty() {
            return Err(AppError::Validation("至少选择一项委托能力".into()));
        }
        let account_id_text = command.account_id.to_string();
        let joined_capabilities = capability_keys.join("\0");
        let requested_expiry = command
            .expires_at
            .map(|value| value.to_rfc3339())
            .unwrap_or_default();
        let fingerprint = request_fingerprint(&[
            account_id_text.as_bytes(),
            joined_capabilities.as_bytes(),
            requested_expiry.as_bytes(),
            reason.as_bytes(),
        ]);
        let idempotency_hash = unkeyed_hash(IDEMPOTENCY_DOMAIN, idempotency_key.as_bytes());
        let txn = self.db.write().begin().await.map_err(database_error)?;
        self.account_repo
            .lock_tenant_in_txn(&txn, tenant_id, ServiceAccountLock::Update)
            .await?;
        let account = self
            .lock_account(&txn, tenant_id, command.account_id)
            .await?;
        if !account.is_enabled() {
            return Err(AppError::Validation("服务账号已停用".into()));
        }
        let user = self
            .user_repo
            .find_by_id_for_update(&txn, tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        if !user.is_enabled() {
            return Err(AppError::Authorization("当前用户已停用".into()));
        }
        if let Some(existing) = self
            .delegation_repo
            .find_idempotent(&txn, tenant_id, actor.user_id, &idempotency_hash)
            .await?
        {
            ensure_same_fingerprint(&existing.request_fingerprint, &fingerprint)?;
            let vo = self.delegation_vo(&txn, existing).await?;
            crate::commit_current_audit(txn).await?;
            return Ok(CreatedDelegationVo {
                delegation: vo,
                token: None,
            });
        }
        let common = self
            .common_capability_keys_in_txn(&txn, tenant_id, command.account_id, actor.user_id)
            .await?;
        if capability_keys.iter().any(|key| !common.contains(key)) {
            return Err(AppError::Authorization(
                "只能委托双方当前共同拥有的已注册能力".into(),
            ));
        }
        let now = database_now(&txn).await?;
        let expires_at = command.expires_at.unwrap_or_else(|| {
            now + Duration::hours(i64::from(self.config.default_delegation_hours))
        });
        if expires_at <= now
            || expires_at > now + Duration::days(i64::from(self.config.max_delegation_days))
        {
            return Err(AppError::Validation(format!(
                "委托有效期不能超过 {} 天",
                self.config.max_delegation_days
            )));
        }
        let issued = IssuedDelegationToken::issue();
        let (pepper_version, pepper) = self.keyring.active();
        let token_mac = issued.mac(pepper)?;
        let model = service_delegation::Model {
            id: crate::next_id()?,
            tenant_id: tenant_id.to_owned(),
            account_id: command.account_id,
            user_id: actor.user_id,
            token_mac,
            pepper_version,
            status: service_delegation::Model::STATUS_ACTIVE.into(),
            version: 1,
            not_before: now,
            expires_at,
            reason,
            created_by_user_id: actor.user_id,
            revoked_at: None,
            revoked_by: None,
            created_at: now,
            updated_at: now,
            idempotency_key_hash: idempotency_hash,
            request_fingerprint: fingerprint,
        };
        let saved = self
            .delegation_repo
            .insert_in_txn(&txn, tenant_id, actor.user_id, model)
            .await?;
        self.delegation_repo
            .replace_capabilities_in_txn(&txn, tenant_id, saved.id, &capability_keys)
            .await?;
        self.account_repo
            .increment_authorization_version_in_txn(&txn, tenant_id, command.account_id)
            .await?;
        let versions = self
            .authorization_cache
            .increment_user_versions_in_transaction(&txn, tenant_id, &[actor.user_id])
            .await?;
        let epoch = self.bump_tenant_epoch(&txn, tenant_id).await?;
        let vo = delegation_vo_with_keys(saved, capability_keys);
        crate::commit_current_audit(txn).await?;
        self.sync_committed_authorization_state(tenant_id, epoch, &versions)
            .await;
        Ok(CreatedDelegationVo {
            delegation: vo,
            token: Some(issued.into_presented()),
        })
    }

    pub async fn list_my_delegations(
        &self,
        actor: &ActorContext,
    ) -> AppResult<Vec<ServiceDelegationVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let rows = self
            .delegation_repo
            .list_for_user(&db, tenant_id, actor.user_id)
            .await?;
        self.delegations_with_capabilities(&db, rows).await
    }

    pub async fn revoke_my_delegation(
        &self,
        actor: &ActorContext,
        delegation_id: i64,
    ) -> AppResult<()> {
        self.revoke_delegation(actor, delegation_id, true).await
    }

    pub async fn list_delegations(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<ServiceDelegationVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let base = service_delegation::Entity::find()
            .filter(service_delegation::Column::TenantId.eq(tenant_id));
        let total = base.clone().count(&db).await.map_err(database_error)?;
        let rows = base
            .order_by_desc(service_delegation::Column::CreatedAt)
            .offset(page.offset())
            .limit(page.page_size())
            .all(&db)
            .await
            .map_err(database_error)?;
        let records = self.delegations_with_capabilities(&db, rows).await?;
        Ok(PageResult::new(records, total, &page))
    }

    pub async fn revoke_managed_delegation(
        &self,
        actor: &ActorContext,
        delegation_id: i64,
    ) -> AppResult<()> {
        self.revoke_delegation(actor, delegation_id, false).await
    }

    pub(super) async fn revoke_delegation(
        &self,
        actor: &ActorContext,
        delegation_id: i64,
        owner_only: bool,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let txn = self.db.write().begin().await.map_err(database_error)?;
        self.account_repo
            .lock_tenant_in_txn(&txn, tenant_id, ServiceAccountLock::Update)
            .await?;
        let delegation_hint = self
            .delegation_repo
            .find_by_id(&txn, tenant_id, delegation_id)
            .await?
            .ok_or_else(|| AppError::NotFound("委托不存在".into()))?;
        self.lock_account(&txn, tenant_id, delegation_hint.account_id)
            .await?;
        self.user_repo
            .find_by_id_for_update(&txn, tenant_id, delegation_hint.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        let delegation = service_delegation::Entity::find_by_id(delegation_id)
            .filter(service_delegation::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(&txn)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("委托不存在".into()))?;
        if owner_only && delegation.user_id != actor.user_id {
            return Err(AppError::Authorization("只能撤销本人创建的委托".into()));
        }
        if !self
            .delegation_repo
            .revoke_in_txn(&txn, tenant_id, delegation_id, actor.user_id)
            .await?
        {
            return Err(AppError::Conflict("委托已撤销".into()));
        }
        self.account_repo
            .increment_authorization_version_in_txn(&txn, tenant_id, delegation.account_id)
            .await?;
        let versions = self
            .authorization_cache
            .increment_user_versions_in_transaction(&txn, tenant_id, &[delegation.user_id])
            .await?;
        let epoch = self.bump_tenant_epoch(&txn, tenant_id).await?;
        crate::commit_current_audit(txn).await?;
        self.sync_committed_authorization_state(tenant_id, epoch, &versions)
            .await;
        Ok(())
    }
}
