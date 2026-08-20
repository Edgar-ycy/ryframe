use super::*;

impl ServiceAccountService {
    pub async fn list_credentials(
        &self,
        actor: &ActorContext,
        account_id: i64,
    ) -> AppResult<Vec<ServiceCredentialVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let credentials = self
            .read
            .enabled_account_credentials(tenant_id, account_id)
            .await?
            .ok_or_else(|| AppError::NotFound("可用的服务账号不存在".into()))?;
        Ok(credentials
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
        let transaction = self.write.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            let mut account = transaction
                .lock_account(tenant_id, account_id)
                .await?
                .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
            if !account.is_enabled() {
                return Err(AppError::Validation("服务账号已停用".into()));
            }
            if let Some(existing) = transaction
                .find_idempotent_credential(tenant_id, account_id, &idempotency_hash)
                .await?
            {
                ensure_same_fingerprint(&existing.request_fingerprint, &fingerprint)?;
                return Ok((existing, None, None));
            }
            let now = transaction.authorization_mirror().database_now().await?;
            let max_expires_at = now + Duration::days(i64::from(self.config.max_credential_days));
            if command.expires_at <= now || command.expires_at > max_expires_at {
                return Err(AppError::Validation(format!(
                    "API Key 必须到期且有效期不能超过 {} 天",
                    self.config.max_credential_days
                )));
            }
            let active = transaction
                .count_active_credentials_at(tenant_id, account_id, now)
                .await?;
            if active >= u64::from(self.config.max_active_credentials) {
                return Err(AppError::Conflict(
                    "每个服务账号最多只能有两把有效 API Key".into(),
                ));
            }
            let issued = IssuedApiKey::issue();
            let (pepper_version, pepper) = self.keyring.active();
            let secret_mac = issued.mac(pepper)?;
            let credential = ServiceCredentialWriteRecord {
                id: crate::next_id()?,
                tenant_id: tenant_id.to_owned(),
                account_id,
                key_id: issued.key_id().to_owned(),
                secret_mac,
                pepper_version,
                label,
                status: ServiceCredentialWriteRecord::STATUS_ACTIVE.into(),
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
            let saved = transaction
                .insert_credential(tenant_id, account_id, credential)
                .await?;
            account.authorization_version = account.authorization_version.saturating_add(1);
            account.updated_at = now;
            transaction.save_account(tenant_id, account).await?;
            let epoch = self
                .bump_tenant_epoch(transaction.authorization_mirror(), tenant_id)
                .await?;
            Ok((saved, Some(issued.into_presented()), Some(epoch)))
        }
        .await;
        match result {
            Ok((saved, secret, epoch)) => {
                transaction.commit().await?;
                if let Some(epoch) = epoch {
                    self.sync_committed_authorization_state(tenant_id, epoch, &[])
                        .await;
                }
                Ok(CreatedCredentialVo {
                    credential: saved.into(),
                    secret,
                })
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn revoke_credential(
        &self,
        actor: &ActorContext,
        account_id: i64,
        credential_id: i64,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let transaction = self.write.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            let mut account = transaction
                .lock_account(tenant_id, account_id)
                .await?
                .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
            let mut credential = transaction
                .lock_credential(tenant_id, account_id, credential_id)
                .await?
                .ok_or_else(|| AppError::NotFound("API Key 不存在".into()))?;
            if credential.status != ServiceCredentialWriteRecord::STATUS_ACTIVE {
                return Err(AppError::Conflict("API Key 已撤销".into()));
            }
            let now = transaction.authorization_mirror().database_now().await?;
            credential.status = ServiceCredentialWriteRecord::STATUS_REVOKED.into();
            credential.revoked_at = Some(now);
            credential.revoked_by = Some(actor.user_id);
            credential.updated_at = now;
            transaction
                .save_credential(tenant_id, account_id, credential)
                .await?;
            account.authorization_version = account.authorization_version.saturating_add(1);
            account.updated_at = now;
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
