use super::*;

impl ServiceAccountService {
    pub async fn list_credentials(
        &self,
        actor: &ActorContext,
        account_id: i64,
    ) -> AppResult<Vec<ServiceCredentialVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        self.account_repo
            .find_by_id(&db, tenant_id, account_id)
            .await?
            .filter(service_account::Model::is_enabled)
            .ok_or_else(|| AppError::NotFound("可用的服务账号不存在".into()))?;
        Ok(self
            .credential_repo
            .list_for_account(&db, tenant_id, account_id)
            .await?
            .into_iter()
            .map(ServiceCredentialVo::from)
            .collect())
    }

    pub async fn create_credential(
        &self,
        actor: &ActorContext,
        account_id: i64,
        command: CreateCredentialCommand,
    ) -> AppResult<CreatedCredentialVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let label = required_text(command.label, "凭据标签", 128)?;
        let idempotency_key = validate_idempotency_key(command.idempotency_key)?;
        let expires_at_text = command.expires_at.to_rfc3339();
        let fingerprint = request_fingerprint(&[label.as_bytes(), expires_at_text.as_bytes()]);
        let idempotency_hash = unkeyed_hash(IDEMPOTENCY_DOMAIN, idempotency_key.as_bytes());
        let txn = self.db.write().begin().await.map_err(database_error)?;
        self.account_repo
            .lock_tenant_in_txn(&txn, tenant_id, ServiceAccountLock::Update)
            .await?;
        let account = self.lock_account(&txn, tenant_id, account_id).await?;
        if !account.is_enabled() {
            return Err(AppError::Validation("服务账号已停用".into()));
        }
        if let Some(existing) = self
            .credential_repo
            .find_idempotent(&txn, tenant_id, account_id, &idempotency_hash)
            .await?
        {
            ensure_same_fingerprint(&existing.request_fingerprint, &fingerprint)?;
            let result = CreatedCredentialVo {
                credential: existing.into(),
                secret: None,
            };
            crate::commit_current_audit(txn).await?;
            return Ok(result);
        }
        let now = database_now(&txn).await?;
        let max_expires_at = now + Duration::days(i64::from(self.config.max_credential_days));
        if command.expires_at <= now || command.expires_at > max_expires_at {
            return Err(AppError::Validation(format!(
                "API Key 必须到期且有效期不能超过 {} 天",
                self.config.max_credential_days
            )));
        }
        let active = self
            .credential_repo
            .count_active_at(&txn, tenant_id, account_id, now)
            .await?;
        if active >= u64::from(self.config.max_active_credentials) {
            return Err(AppError::Conflict(
                "每个服务账号最多只能有两把有效 API Key".into(),
            ));
        }
        let issued = IssuedApiKey::issue();
        let (pepper_version, pepper) = self.keyring.active();
        let secret_mac = issued.mac(pepper)?;
        let model = service_credential::Model {
            id: crate::next_id()?,
            tenant_id: tenant_id.to_owned(),
            account_id,
            key_id: issued.key_id().to_owned(),
            secret_mac,
            pepper_version,
            label,
            status: service_credential::Model::STATUS_ACTIVE.into(),
            expires_at: command.expires_at,
            last_used_at: None,
            created_by: actor.user_id,
            revoked_at: None,
            revoked_by: None,
            created_at: now,
            updated_at: now,
            idempotency_key_hash: idempotency_hash,
            request_fingerprint: fingerprint,
        };
        let saved = self
            .credential_repo
            .insert_in_txn(&txn, tenant_id, account_id, model)
            .await?;
        self.account_repo
            .increment_authorization_version_in_txn(&txn, tenant_id, account_id)
            .await?;
        let epoch = self.bump_tenant_epoch(&txn, tenant_id).await?;
        crate::commit_current_audit(txn).await?;
        self.sync_committed_authorization_state(tenant_id, epoch, &[])
            .await;
        Ok(CreatedCredentialVo {
            credential: saved.into(),
            secret: Some(issued.into_presented()),
        })
    }

    pub async fn revoke_credential(
        &self,
        actor: &ActorContext,
        account_id: i64,
        credential_id: i64,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let txn = self.db.write().begin().await.map_err(database_error)?;
        self.account_repo
            .lock_tenant_in_txn(&txn, tenant_id, ServiceAccountLock::Update)
            .await?;
        self.lock_account(&txn, tenant_id, account_id).await?;
        self.credential_repo
            .find_by_id(&txn, tenant_id, account_id, credential_id)
            .await?
            .ok_or_else(|| AppError::NotFound("API Key 不存在".into()))?;
        if !self
            .credential_repo
            .revoke_in_txn(&txn, tenant_id, account_id, credential_id, actor.user_id)
            .await?
        {
            return Err(AppError::Conflict("API Key 已撤销".into()));
        }
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
