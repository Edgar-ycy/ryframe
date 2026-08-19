use super::*;

impl ServiceAccountService {
    pub async fn list_accounts(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<ServiceAccountVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let result = self.account_repo.find_by_page(&db, tenant_id, page).await?;
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
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let account = self
            .account_repo
            .find_by_id(&db, tenant_id, account_id)
            .await?
            .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
        let role_ids = self
            .account_repo
            .role_ids(&db, tenant_id, account_id)
            .await?
            .into_iter()
            .map(|id| id.to_string())
            .collect();
        Ok(ServiceAccountDetailVo {
            account: account.into(),
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
        let txn = self.db.write().begin().await.map_err(database_error)?;
        self.account_repo
            .lock_tenant_in_txn(&txn, tenant_id, ServiceAccountLock::Update)
            .await?;
        if service_account::Entity::find()
            .filter(service_account::Column::TenantId.eq(tenant_id))
            .filter(service_account::Column::Code.eq(&code))
            .one(&txn)
            .await
            .map_err(database_error)?
            .is_some()
        {
            return Err(AppError::Conflict("服务账号代码已存在且不能复用".into()));
        }
        validate_dept(&txn, tenant_id, command.dept_id).await?;
        let now = database_now(&txn).await?;
        let model = service_account::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            code,
            name,
            description,
            dept_id: command.dept_id,
            status: service_account::Model::STATUS_NORMAL.into(),
            authorization_version: 1,
            max_requests_per_minute,
            created_by: actor.user_id,
            del_flag: service_account::Model::DEL_FLAG_NORMAL.into(),
            created_at: now,
            updated_at: now,
        };
        let saved = self
            .account_repo
            .insert_in_txn(&txn, tenant_id, model)
            .await?;
        let epoch = self.bump_tenant_epoch(&txn, tenant_id).await?;
        crate::commit_current_audit(txn).await?;
        self.sync_committed_authorization_state(tenant_id, epoch, &[])
            .await;
        Ok(saved.into())
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
        let txn = self.db.write().begin().await.map_err(database_error)?;
        self.account_repo
            .lock_tenant_in_txn(&txn, tenant_id, ServiceAccountLock::Update)
            .await?;
        let mut account = self.lock_account(&txn, tenant_id, account_id).await?;
        validate_dept(&txn, tenant_id, command.dept_id).await?;
        account.name = name;
        account.description = description;
        account.dept_id = command.dept_id;
        account.max_requests_per_minute = command.max_requests_per_minute;
        account.authorization_version = account.authorization_version.saturating_add(1);
        account.updated_at = database_now(&txn).await?;
        let saved = self
            .account_repo
            .update_in_txn(&txn, tenant_id, account)
            .await?;
        let epoch = self.bump_tenant_epoch(&txn, tenant_id).await?;
        crate::commit_current_audit(txn).await?;
        self.sync_committed_authorization_state(tenant_id, epoch, &[])
            .await;
        Ok(saved.into())
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
            service_account::Model::STATUS_NORMAL | service_account::Model::STATUS_DISABLED
        ) {
            return Err(AppError::Validation("无效的服务账号状态".into()));
        }
        let txn = self.db.write().begin().await.map_err(database_error)?;
        self.account_repo
            .lock_tenant_in_txn(&txn, tenant_id, ServiceAccountLock::Update)
            .await?;
        let mut account = self.lock_account(&txn, tenant_id, account_id).await?;
        account.status = status;
        account.authorization_version = account.authorization_version.saturating_add(1);
        account.updated_at = database_now(&txn).await?;
        self.account_repo
            .update_in_txn(&txn, tenant_id, account)
            .await?;
        let epoch = self.bump_tenant_epoch(&txn, tenant_id).await?;
        crate::commit_current_audit(txn).await?;
        self.sync_committed_authorization_state(tenant_id, epoch, &[])
            .await;
        Ok(())
    }

    pub async fn delete_account(&self, actor: &ActorContext, account_id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let txn = self.db.write().begin().await.map_err(database_error)?;
        self.account_repo
            .lock_tenant_in_txn(&txn, tenant_id, ServiceAccountLock::Update)
            .await?;
        self.lock_account(&txn, tenant_id, account_id).await?;
        let now = database_now(&txn).await?;
        let result = service_account::Entity::update_many()
            .col_expr(
                service_account::Column::DelFlag,
                Expr::value(service_account::Model::DEL_FLAG_DELETED),
            )
            .col_expr(
                service_account::Column::AuthorizationVersion,
                Expr::col(service_account::Column::AuthorizationVersion).add(1),
            )
            .col_expr(service_account::Column::UpdatedAt, Expr::value(now))
            .filter(service_account::Column::TenantId.eq(tenant_id))
            .filter(service_account::Column::Id.eq(account_id))
            .exec(&txn)
            .await
            .map_err(database_error)?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("服务账号不存在".into()));
        }
        let epoch = self.bump_tenant_epoch(&txn, tenant_id).await?;
        crate::commit_current_audit(txn).await?;
        self.sync_committed_authorization_state(tenant_id, epoch, &[])
            .await;
        Ok(())
    }
}
