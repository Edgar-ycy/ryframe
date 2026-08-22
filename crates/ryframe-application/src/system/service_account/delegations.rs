use super::*;

impl ServiceAccountService {
    pub async fn common_delegated_capabilities(
        &self,
        actor: &ActorContext,
        account_id: i64,
    ) -> AppResult<Vec<ServiceCapabilityDescriptor>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let snapshot = self
            .authorization_read
            .permission_snapshot(tenant_id, actor.user_id, account_id)
            .await?
            .ok_or_else(|| AppError::NotFound("可用的服务账号不存在".into()))?;
        Ok(common_capabilities(
            &self.capabilities,
            &snapshot.user_permissions,
            &snapshot.account_permissions,
        ))
    }

    /// 个人委托页可见的候选账号，不要求服务账号管理权限。
    pub async fn delegation_targets(
        &self,
        actor: &ActorContext,
    ) -> AppResult<Vec<ServiceDelegationTargetVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        const MAX_DELEGATION_TARGETS: u64 = 1_000;
        let target_set = self
            .authorization_read
            .delegation_targets(tenant_id, actor.user_id, MAX_DELEGATION_TARGETS)
            .await?;
        let mut targets = Vec::new();
        for account in target_set.accounts {
            let capabilities = common_capabilities(
                &self.capabilities,
                &target_set.user_permissions,
                &account.permission_codes,
            );
            if !capabilities.is_empty() {
                targets.push(ServiceDelegationTargetVo {
                    account_id: account.account_id.to_string(),
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
        let transaction = self.write.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            let mut account = transaction
                .lock_account(tenant_id, command.account_id)
                .await?
                .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
            if !account.is_enabled() {
                return Err(AppError::Validation("服务账号已停用".into()));
            }
            let user = transaction
                .lock_user(tenant_id, actor.user_id)
                .await?
                .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
            if !user.is_enabled() {
                return Err(AppError::Authorization("当前用户已停用".into()));
            }
            if let Some(existing) = transaction
                .find_idempotent_delegation(tenant_id, actor.user_id, &idempotency_hash)
                .await?
            {
                ensure_same_fingerprint(&existing.request_fingerprint, &fingerprint)?;
                return Ok((existing, None, None, Vec::new()));
            }
            let snapshot = transaction
                .permission_snapshot(tenant_id, command.account_id, actor.user_id)
                .await?;
            let common = common_capabilities(
                &self.capabilities,
                &snapshot.user_permissions,
                &snapshot.account_permissions,
            )
            .into_iter()
            .map(|capability| capability.key)
            .collect::<HashSet<_>>();
            if capability_keys.iter().any(|key| !common.contains(key)) {
                return Err(AppError::Authorization(
                    "只能委托双方当前共同拥有的已注册能力".into(),
                ));
            }
            let now = transaction.authorization_mirror().database_now().await?;
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
            let delegation = ServiceDelegationWriteRecord {
                id: crate::next_id()?,
                tenant_id: tenant_id.to_owned(),
                account_id: command.account_id,
                user_id: actor.user_id,
                token_mac,
                pepper_version,
                status: ServiceDelegationWriteRecord::STATUS_ACTIVE.into(),
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
                capability_keys,
            };
            let saved = transaction
                .insert_delegation(tenant_id, actor.user_id, delegation)
                .await?;
            account.authorization_version = account.authorization_version.saturating_add(1);
            account.updated_at = now;
            transaction.save_account(tenant_id, account).await?;
            let versions = self
                .authorization_cache
                .increment_user_versions_in_transaction(
                    transaction.authorization_mirror(),
                    tenant_id,
                    &[actor.user_id],
                )
                .await?;
            let epoch = self
                .bump_tenant_epoch(transaction.authorization_mirror(), tenant_id)
                .await?;
            Ok((saved, Some(issued.into_presented()), Some(epoch), versions))
        }
        .await;
        match result {
            Ok((saved, token, epoch, versions)) => {
                transaction.commit().await?;
                if let Some(epoch) = epoch {
                    self.sync_committed_authorization_state(tenant_id, epoch, &versions)
                        .await;
                }
                Ok(CreatedDelegationVo {
                    delegation: saved.into(),
                    token,
                })
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn list_my_delegations(
        &self,
        actor: &ActorContext,
    ) -> AppResult<Vec<ServiceDelegationVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        self.read
            .delegations_for_user(tenant_id, actor.user_id)
            .await
            .map(|rows| rows.into_iter().map(ServiceDelegationVo::from).collect())
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
        let result = self.read.list_delegations(tenant_id, page).await?;
        Ok(PageResult {
            records: result
                .records
                .into_iter()
                .map(ServiceDelegationVo::from)
                .collect(),
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
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
        let transaction = self.write.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            let hint = transaction
                .delegation_identity(tenant_id, delegation_id)
                .await?
                .ok_or_else(|| AppError::NotFound("委托不存在".into()))?;
            let mut account = transaction
                .lock_account(tenant_id, hint.account_id)
                .await?
                .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
            transaction
                .lock_user(tenant_id, hint.user_id)
                .await?
                .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
            let mut delegation = transaction
                .lock_delegation(tenant_id, delegation_id)
                .await?
                .ok_or_else(|| AppError::NotFound("委托不存在".into()))?;
            if owner_only && delegation.user_id != actor.user_id {
                return Err(AppError::Authorization("只能撤销本人创建的委托".into()));
            }
            if delegation.status != ServiceDelegationWriteRecord::STATUS_ACTIVE {
                return Err(AppError::Conflict("委托已撤销".into()));
            }
            let now = transaction.authorization_mirror().database_now().await?;
            delegation.status = ServiceDelegationWriteRecord::STATUS_REVOKED.into();
            delegation.version = delegation.version.saturating_add(1);
            delegation.revoked_at = Some(now);
            delegation.revoked_by = Some(actor.user_id);
            delegation.updated_at = now;
            let delegation = transaction.save_delegation(tenant_id, delegation).await?;
            account.authorization_version = account.authorization_version.saturating_add(1);
            account.updated_at = now;
            transaction.save_account(tenant_id, account).await?;
            let versions = self
                .authorization_cache
                .increment_user_versions_in_transaction(
                    transaction.authorization_mirror(),
                    tenant_id,
                    &[delegation.user_id],
                )
                .await?;
            let epoch = self
                .bump_tenant_epoch(transaction.authorization_mirror(), tenant_id)
                .await?;
            Ok((epoch, versions))
        }
        .await;
        match result {
            Ok((epoch, versions)) => {
                transaction.commit().await?;
                self.sync_committed_authorization_state(tenant_id, epoch, &versions)
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
