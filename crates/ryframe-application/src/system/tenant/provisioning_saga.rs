use ryframe_kernel::{ActorContext, AppError, AppResult};

use super::{
    CreateTenantParams, TenantService, TenantVo,
    provisioning::provision_new_tenant_in_transaction,
    validation::{
        provisioning_switch_token, validate_data_target_key, validate_idempotency_key,
        validate_tenant_limits,
    },
};
use crate::ports::tenants::{
    ProvisionTenantRecord, TENANT_STATUS_ENABLED, TENANT_STATUS_PROVISIONING,
    TENANT_STATUS_PROVISIONING_FAILED, TenantProvisioningPlacement, TenantTransaction,
};

#[derive(Clone, Copy, Debug)]
struct ValidatedTenantQuota {
    max_users: i32,
    max_roles: i32,
    max_storage_mb: i64,
    max_requests_per_minute: i32,
}

impl ValidatedTenantQuota {
    fn from_create_params(params: &CreateTenantParams) -> AppResult<Self> {
        let quota = Self {
            max_users: params.max_users.unwrap_or(100),
            max_roles: params.max_roles.unwrap_or(20),
            max_storage_mb: params.max_storage_mb.unwrap_or(1024),
            max_requests_per_minute: params.max_requests_per_min.unwrap_or(1000),
        };
        validate_tenant_limits(
            quota.max_users,
            quota.max_roles,
            quota.max_storage_mb,
            quota.max_requests_per_minute,
        )?;
        Ok(quota)
    }
}

fn build_provision_tenant_record(
    params: &CreateTenantParams,
    quota: &ValidatedTenantQuota,
    pending: &TenantProvisioningPlacement,
    resources: crate::ports::product::ProvisioningCapabilityResources,
) -> AppResult<ProvisionTenantRecord> {
    Ok(ProvisionTenantRecord {
        provisioning_request_token: pending.switch_token.clone(),
        tenant_id: params.tenant_id.clone(),
        name: params.name.clone(),
        domain: params.domain.clone(),
        expire_at: params.expire_at,
        max_users: quota.max_users,
        max_roles: quota.max_roles,
        max_storage_mb: quota.max_storage_mb,
        max_requests_per_minute: quota.max_requests_per_minute,
        admin_username: params.admin_username.clone(),
        admin_password_hash: ryframe_auth::password::hash(&params.admin_password)?,
        // 首事务仅创建租户身份与非能力模板；Capability 资源必须等目标库
        // fence 成功后再同步，避免数据面尚未就绪时暴露模块入口。
        enabled_capability_route_keys: Vec::new(),
        enabled_capability_permission_codes: Vec::new(),
        managed_capability_route_keys: resources.managed_route_keys,
        managed_capability_permission_codes: resources.managed_permission_codes,
        default_admin_permission_codes: Vec::new(),
    })
}

impl TenantService {
    pub async fn create(
        &self,
        actor: &ActorContext,
        params: CreateTenantParams,
    ) -> AppResult<TenantVo> {
        super::ensure_platform_admin(actor)?;
        ryframe_kernel::TenantId::parse(&params.tenant_id)?;
        validate_data_target_key(&params.data_target_key)?;
        validate_idempotency_key(&params.idempotency_key)?;
        ryframe_auth::password::validate_complexity(&params.admin_password)?;
        let quota = ValidatedTenantQuota::from_create_params(&params)?;
        let tenant_id = params.tenant_id.clone();
        // switch_token 持久化在 pending placement 中，作为控制库权威的幂等键与
        // 非敏感请求指纹；管理员密码另由已持久化的 Argon2 摘要强校验。
        let switch_token = provisioning_switch_token(
            &params,
            quota.max_users,
            quota.max_roles,
            quota.max_storage_mb,
            quota.max_requests_per_minute,
        );
        let pending = self.tenant_provisioning.prepare(
            tenant_id.clone(),
            params.data_target_key.clone(),
            1,
            switch_token,
        )?;
        let already_enabled = self
            .begin_or_resume_provisioning(actor, &params, &quota, &pending)
            .await?;
        if already_enabled {
            return self.find_tenant_vo(&tenant_id).await;
        }

        self.complete_provisioning_saga(&params, &pending).await?;
        self.find_tenant_vo(&tenant_id).await
    }

    async fn begin_or_resume_provisioning(
        &self,
        actor: &ActorContext,
        params: &CreateTenantParams,
        quota: &ValidatedTenantQuota,
        pending: &TenantProvisioningPlacement,
    ) -> AppResult<bool> {
        let transaction = self.persistence.begin().await?;
        let operation = async {
            if transaction
                .lock_optional_tenant(&params.tenant_id)
                .await?
                .is_some()
            {
                return self
                    .resume_provisioning_in_txn(transaction.as_ref(), params, quota, pending)
                    .await;
            }
            self.provision_new_tenant(transaction.as_ref(), actor, params, quota, pending)
                .await?;
            Ok(false)
        };
        let result = operation.await;
        crate::complete_transaction(
            transaction,
            result,
            crate::TransactionAuditMode::CurrentRequest,
        )
        .await
    }

    async fn complete_provisioning_saga(
        &self,
        params: &CreateTenantParams,
        pending: &TenantProvisioningPlacement,
    ) -> AppResult<()> {
        if let Err(error) = self.tenant_provisioning.provision_fence(pending).await {
            self.mark_provisioning_failed(pending).await;
            return Err(error);
        }

        if let Err(error) = self
            .sync_provisioning_resources(pending, params.plan_version_id)
            .await
        {
            self.mark_provisioning_failed(pending).await;
            return Err(error);
        }

        let finalization = self.finalize_provisioning(pending).await;
        if let Err(error) = finalization {
            self.mark_provisioning_failed(pending).await;
            return Err(error);
        }
        Ok(())
    }

    async fn provision_new_tenant(
        &self,
        transaction: &dyn TenantTransaction,
        actor: &ActorContext,
        params: &CreateTenantParams,
        quota: &ValidatedTenantQuota,
        pending: &TenantProvisioningPlacement,
    ) -> AppResult<()> {
        let resources = self
            .product
            .provisioning_resources_in_txn(transaction.product(), params.plan_version_id)
            .await?;
        let record = build_provision_tenant_record(params, quota, pending, resources)?;
        provision_new_tenant_in_transaction(transaction, &record).await?;
        transaction
            .assign_initial_product(&params.tenant_id, params.plan_version_id, actor.user_id)
            .await?;
        transaction.create_pending(pending).await
    }

    async fn find_tenant_vo(&self, tenant_id: &str) -> AppResult<TenantVo> {
        self.persistence
            .find(tenant_id)
            .await?
            .map(TenantVo::from)
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))
    }

    async fn resume_provisioning_in_txn(
        &self,
        transaction: &dyn TenantTransaction,
        params: &CreateTenantParams,
        quota: &ValidatedTenantQuota,
        pending: &TenantProvisioningPlacement,
    ) -> AppResult<bool> {
        let existing = transaction.lock_tenant(&params.tenant_id).await?;
        let request = transaction
            .lock_provision_request(&params.tenant_id)
            .await?
            .ok_or_else(|| {
                AppError::Conflict("现有租户没有创建 Saga 幂等记录，不能作为创建请求续跑".into())
            })?;
        if request.request_token != pending.switch_token {
            return Err(AppError::Conflict(
                "Idempotency-Key 已用于不同的租户创建请求".into(),
            ));
        }
        if !ryframe_auth::password::verify(&params.admin_password, &request.admin_password_hash)? {
            return Err(AppError::Conflict(
                "Idempotency-Key 已用于不同的租户创建请求".into(),
            ));
        }
        // 创建完成后的套餐、数据放置、管理员密码和基础资料允许由独立业务继续变更；
        // 权威请求 token 匹配即证明这是原创建请求的持久幂等重放。
        if existing.status == TENANT_STATUS_ENABLED {
            return Ok(true);
        }
        if existing.name != params.name
            || existing.domain != params.domain
            || existing.expire_at != params.expire_at
            || existing.max_users != quota.max_users
            || existing.max_roles != quota.max_roles
            || existing.max_storage_mb != quota.max_storage_mb
            || existing.max_requests_per_min != quota.max_requests_per_minute
        {
            return Err(AppError::Conflict(
                "租户创建重试参数与已持久化 provisioning 请求不一致".into(),
            ));
        }
        if !matches!(
            existing.status.as_str(),
            TENANT_STATUS_PROVISIONING | TENANT_STATUS_PROVISIONING_FAILED | TENANT_STATUS_ENABLED
        ) {
            return Err(AppError::Conflict("现有租户状态不允许恢复创建 Saga".into()));
        }
        let assignment = transaction
            .product_assignment(&params.tenant_id)
            .await?
            .ok_or_else(|| AppError::Conflict("现有租户缺少 provisioning 套餐快照".into()))?;
        if assignment.plan_version_id != params.plan_version_id {
            return Err(AppError::Conflict(
                "租户创建重试的 plan_version_id 与已持久化请求不一致".into(),
            ));
        }
        let admin = transaction
            .find_admin(&params.tenant_id, &params.admin_username)
            .await?
            .ok_or_else(|| AppError::Conflict("租户创建重试的管理员账号不匹配".into()))?;
        if !ryframe_auth::password::verify(&params.admin_password, &admin.password_hash)? {
            return Err(AppError::Conflict(
                "租户创建重试的管理员凭据与已持久化请求不一致".into(),
            ));
        }
        transaction.create_or_resume_pending(pending).await?;
        if existing.status == TENANT_STATUS_PROVISIONING_FAILED {
            transaction
                .update_status(&params.tenant_id, TENANT_STATUS_PROVISIONING)
                .await?;
        }
        Ok(false)
    }

    async fn sync_provisioning_resources(
        &self,
        pending: &TenantProvisioningPlacement,
        plan_version_id: i64,
    ) -> AppResult<()> {
        let transaction = self.persistence.begin().await?;
        let tenant = transaction.lock_tenant(&pending.tenant_id).await?;
        if tenant.status != TENANT_STATUS_PROVISIONING {
            return Err(AppError::TenantOperationConflict(
                "租户已不处于 provisioning，不能同步初始化能力资源".into(),
            ));
        }
        self.product
            .sync_provisioning_resources_in_txn(
                transaction.product(),
                &pending.tenant_id,
                plan_version_id,
            )
            .await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await
    }

    async fn finalize_provisioning(&self, pending: &TenantProvisioningPlacement) -> AppResult<()> {
        let transaction = self.persistence.begin().await?;
        let tenant = transaction.lock_tenant(&pending.tenant_id).await?;
        if tenant.status == TENANT_STATUS_ENABLED {
            transaction.activate_placement(pending).await?;
            return transaction.commit(crate::TransactionAuditMode::Skip).await;
        }
        if tenant.status != TENANT_STATUS_PROVISIONING {
            return Err(AppError::Conflict(
                "租户 provisioning 状态已变化，无法完成启用".into(),
            ));
        }
        transaction.activate_placement(pending).await?;
        transaction
            .update_status(&pending.tenant_id, TENANT_STATUS_ENABLED)
            .await?;
        transaction.commit(crate::TransactionAuditMode::Skip).await
    }

    async fn mark_provisioning_failed(&self, pending: &TenantProvisioningPlacement) {
        let Ok(transaction) = self.persistence.begin().await else {
            tracing::error!(tenant_id = %pending.tenant_id, "无法开启租户 provisioning 失败补偿事务");
            return;
        };
        let result = async {
            let tenant = transaction.lock_tenant(&pending.tenant_id).await?;
            if tenant.status == TENANT_STATUS_ENABLED {
                return Ok(());
            }
            transaction.fail_placement(pending).await?;
            transaction
                .update_status(&pending.tenant_id, TENANT_STATUS_PROVISIONING_FAILED)
                .await
        }
        .await;
        match result {
            Ok(()) => {
                if let Err(error) = transaction.commit(crate::TransactionAuditMode::Skip).await {
                    tracing::error!(tenant_id = %pending.tenant_id, %error, "租户 provisioning 失败补偿提交失败");
                }
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                tracing::error!(tenant_id = %pending.tenant_id, %error, "租户 provisioning 失败补偿执行失败");
            }
        }
    }
}
