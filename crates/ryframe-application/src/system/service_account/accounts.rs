use super::*;

impl ServiceAccountService {
    pub async fn list_accounts(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<ServiceAccountVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let result = self.read.list_accounts(tenant_id, page).await?;
        Ok(PageResult {
            records: result
                .records
                .into_iter()
                .map(ServiceAccountVo::from)
                .collect(),
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }

    pub async fn account_detail(
        &self,
        actor: &ActorContext,
        account_id: i64,
    ) -> AppResult<ServiceAccountDetailVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let detail = self
            .read
            .account_detail(tenant_id, account_id)
            .await?
            .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
        let role_ids = detail
            .role_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect();
        Ok(ServiceAccountDetailVo {
            account: detail.account.into(),
            role_ids,
        })
    }

    pub async fn create_account(
        &self,
        actor: &ActorContext,
        command: CreateServiceAccountCommand,
    ) -> AppResult<ServiceAccountVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let code = validate_code(command.code)?;
        let name = required_text(command.name, "服务账号名称", 128)?;
        let description = optional_text(command.description, 500)?;
        let max_requests_per_minute = command.max_requests_per_minute.unwrap_or_else(|| {
            i32::try_from(self.config.default_requests_per_minute).unwrap_or(i32::MAX)
        });
        validate_rate_limit(max_requests_per_minute)?;
        let transaction = self.write.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            if transaction.account_code_exists(tenant_id, &code).await? {
                return Err(AppError::Conflict("服务账号代码已存在且不能复用".into()));
            }
            validate_dept(transaction.as_ref(), tenant_id, command.dept_id).await?;
            let now = transaction.authorization_mirror().database_now().await?;
            let account = ServiceAccountRecord {
                id: crate::next_id()?,
                tenant_id: tenant_id.to_owned(),
                code,
                name,
                description,
                dept_id: command.dept_id,
                status: ServiceAccountRecord::STATUS_NORMAL.into(),
                authorization_version: 1,
                max_requests_per_minute,
                created_by: actor.user_id,
                deleted: false,
                created_at: now,
                updated_at: now,
            };
            let saved = transaction.insert_account(tenant_id, account).await?;
            let epoch = self
                .bump_tenant_epoch(transaction.authorization_mirror(), tenant_id)
                .await?;
            Ok::<_, AppError>((saved, epoch))
        }
        .await;
        match result {
            Ok((saved, epoch)) => {
                transaction.commit().await?;
                self.sync_committed_authorization_state(tenant_id, epoch, &[])
                    .await;
                Ok(saved.into())
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn update_account(
        &self,
        actor: &ActorContext,
        account_id: i64,
        command: UpdateServiceAccountCommand,
    ) -> AppResult<ServiceAccountVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let name = required_text(command.name, "服务账号名称", 128)?;
        let description = optional_text(command.description, 500)?;
        validate_rate_limit(command.max_requests_per_minute)?;
        let transaction = self.write.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            let mut account = transaction
                .lock_account(tenant_id, account_id)
                .await?
                .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
            validate_dept(transaction.as_ref(), tenant_id, command.dept_id).await?;
            account.name = name;
            account.description = description;
            account.dept_id = command.dept_id;
            account.max_requests_per_minute = command.max_requests_per_minute;
            account.authorization_version = account.authorization_version.saturating_add(1);
            account.updated_at = transaction.authorization_mirror().database_now().await?;
            let saved = transaction.save_account(tenant_id, account).await?;
            let epoch = self
                .bump_tenant_epoch(transaction.authorization_mirror(), tenant_id)
                .await?;
            Ok::<_, AppError>((saved, epoch))
        }
        .await;
        match result {
            Ok((saved, epoch)) => {
                transaction.commit().await?;
                self.sync_committed_authorization_state(tenant_id, epoch, &[])
                    .await;
                Ok(saved.into())
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn update_account_status(
        &self,
        actor: &ActorContext,
        account_id: i64,
        status: String,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        if !matches!(
            status.as_str(),
            ServiceAccountRecord::STATUS_NORMAL | ServiceAccountRecord::STATUS_DISABLED
        ) {
            return Err(AppError::Validation("无效的服务账号状态".into()));
        }
        let transaction = self.write.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            let mut account = transaction
                .lock_account(tenant_id, account_id)
                .await?
                .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
            account.status = status;
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

    pub async fn delete_account(&self, actor: &ActorContext, account_id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let transaction = self.write.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            let mut account = transaction
                .lock_account(tenant_id, account_id)
                .await?
                .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
            account.deleted = true;
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
