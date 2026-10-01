pub mod config_package;
pub mod config_transfer;
pub mod data_migration;
mod login_choices;
mod provisioning;
mod provisioning_saga;
pub mod usage;
mod validation;

pub use login_choices::{LoginTenantChoice, LoginTenantPage};

use std::sync::Arc;

use chrono::{DateTime, Utc};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use serde::Serialize;

use validation::*;

use super::product::ProductService;
use crate::{
    AuthorizationCache,
    ports::tenants::{
        TENANT_STATUS_DISABLED, TENANT_STATUS_ENABLED, TenantPersistencePort,
        TenantProvisioningPort, TenantRecord,
    },
};

const SYSTEM_TENANT_ID: &str = "system";

#[derive(Debug, Serialize)]
pub struct TenantVo {
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
}

impl From<TenantRecord> for TenantVo {
    fn from(tenant: TenantRecord) -> Self {
        Self {
            tenant_id: tenant.tenant_id,
            name: tenant.name,
            domain: tenant.domain,
            status: tenant.status,
            expire_at: tenant.expire_at,
            max_users: tenant.max_users,
            max_roles: tenant.max_roles,
            max_storage_mb: tenant.max_storage_mb,
            max_requests_per_min: tenant.max_requests_per_min,
        }
    }
}

#[derive(Clone)]
pub struct CreateTenantParams {
    /// 原始 Idempotency-Key 只在内存中用于请求 HMAC，不得落库或写日志。
    pub idempotency_key: String,
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: Option<i32>,
    pub max_roles: Option<i32>,
    pub max_storage_mb: Option<i64>,
    pub max_requests_per_min: Option<i32>,
    pub admin_username: String,
    pub admin_password: String,
    pub plan_version_id: i64,
    pub data_target_key: String,
}

#[derive(Debug, Clone)]
pub struct UpdateTenantParams {
    pub name: String,
    pub domain: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
}

pub struct TenantService {
    persistence: Arc<dyn TenantPersistencePort>,
    product: Arc<ProductService>,
    tenant_provisioning: Arc<dyn TenantProvisioningPort>,
    authorization_cache: AuthorizationCache,
}

impl TenantService {
    pub fn new(
        persistence: Arc<dyn TenantPersistencePort>,
        authorization_cache: AuthorizationCache,
        product: Arc<ProductService>,
        tenant_provisioning: Arc<dyn TenantProvisioningPort>,
    ) -> Self {
        Self {
            persistence,
            product,
            tenant_provisioning,
            authorization_cache,
        }
    }

    pub async fn list(&self, actor: &ActorContext) -> AppResult<Vec<TenantVo>> {
        ensure_platform_admin(actor)?;
        self.persistence
            .list()
            .await
            .map(|tenants| tenants.into_iter().map(TenantVo::from).collect())
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        params: UpdateTenantParams,
    ) -> AppResult<TenantVo> {
        ensure_platform_admin(actor)?;
        validate_tenant_limits(
            params.max_users,
            params.max_roles,
            params.max_storage_mb,
            params.max_requests_per_min,
        )?;
        let transaction = self.persistence.begin().await?;
        let mut tenant = transaction
            .lock_tenant_with_limits(
                tenant_id,
                params.max_users,
                params.max_roles,
                params.max_storage_mb,
            )
            .await?;
        if !matches!(
            tenant.status.as_str(),
            TENANT_STATUS_ENABLED | TENANT_STATUS_DISABLED
        ) {
            return Err(AppError::TenantOperationConflict(
                "provisioning 租户只能由创建 Saga 更新，不能修改普通租户资料".into(),
            ));
        }
        if params.expire_at != tenant.expire_at {
            tenant.session_version = tenant.session_version.saturating_add(1);
        }
        tenant.name = params.name;
        tenant.domain = params.domain;
        tenant.expire_at = params.expire_at;
        tenant.max_users = params.max_users;
        tenant.max_roles = params.max_roles;
        tenant.max_storage_mb = params.max_storage_mb;
        tenant.max_requests_per_min = params.max_requests_per_min;
        tenant.updated_at = Utc::now();

        let saved = transaction.save_tenant(tenant).await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(transaction.authorization_mirror(), tenant_id)
            .await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(TenantVo::from(saved))
    }

    pub async fn update_status(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        status: String,
    ) -> AppResult<()> {
        ensure_platform_admin(actor)?;
        if tenant_id == SYSTEM_TENANT_ID {
            return Err(AppError::Validation("不能停用 system 租户".into()));
        }
        if !matches!(
            status.as_str(),
            TENANT_STATUS_ENABLED | TENANT_STATUS_DISABLED
        ) {
            return Err(AppError::Validation(
                "租户状态只能切换为 enabled 或 disabled".into(),
            ));
        }
        let transaction = self.persistence.begin().await?;
        let current = transaction.lock_tenant(tenant_id).await?;
        if !matches!(
            current.status.as_str(),
            TENANT_STATUS_ENABLED | TENANT_STATUS_DISABLED
        ) {
            return Err(AppError::Conflict(
                "provisioning 租户只能由创建 Saga 完成或重试，不能直接切换状态".into(),
            ));
        }
        if current.status == status {
            return transaction
                .commit(crate::TransactionAuditMode::CurrentRequest)
                .await;
        }
        transaction.update_status(tenant_id, &status).await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(transaction.authorization_mirror(), tenant_id)
            .await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await
    }
}
