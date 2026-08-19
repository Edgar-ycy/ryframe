use std::sync::Arc;

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use ryframe_db::{
    ControlDatabaseCluster, ProductRepository, ProvisionTenantCommand, ReadConsistency,
    TenantProvisioningRepository, TenantRepository, entities::tenant,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_tenant_db::{
    PendingTenantDataPlacement, TenantDataError, TenantDataPlacementRepository,
    TenantDatabaseRouter,
};
use sea_orm::{ActiveModelTrait, TransactionTrait};
use serde::Serialize;
use sha2::Sha256;

use super::ProductService;
use crate::AuthorizationCache;

const SYSTEM_TENANT_ID: &str = "system";
const MIN_TENANT_USERS: i32 = 1;
const MIN_TENANT_ROLES: i32 = 2;

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

impl From<tenant::Model> for TenantVo {
    fn from(tenant: tenant::Model) -> Self {
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
    db: ControlDatabaseCluster,
    tenant_repo: TenantRepository,
    provisioning_repo: TenantProvisioningRepository,
    product_repo: ProductRepository,
    product_service: Arc<ProductService>,
    tenant_data: Arc<TenantDatabaseRouter>,
    authorization_cache: AuthorizationCache,
}

impl TenantService {
    pub fn new(
        db: ControlDatabaseCluster,
        authorization_cache: AuthorizationCache,
        product_service: Arc<ProductService>,
        tenant_data: Arc<TenantDatabaseRouter>,
    ) -> Self {
        Self {
            db,
            tenant_repo: TenantRepository,
            provisioning_repo: TenantProvisioningRepository,
            product_repo: ProductRepository,
            product_service,
            tenant_data,
            authorization_cache,
        }
    }

    pub async fn list(&self, actor: &ActorContext) -> AppResult<Vec<TenantVo>> {
        ensure_platform_admin(actor)?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        self.tenant_repo
            .list_all(&db)
            .await
            .map(|tenants| tenants.into_iter().map(TenantVo::from).collect())
    }

    pub async fn create(
        &self,
        actor: &ActorContext,
        params: CreateTenantParams,
    ) -> AppResult<TenantVo> {
        ensure_platform_admin(actor)?;
        ryframe_kernel::TenantId::parse(&params.tenant_id)?;
        validate_data_target_key(&params.data_target_key)?;
        validate_idempotency_key(&params.idempotency_key)?;
        ryframe_auth::password::validate_complexity(&params.admin_password)?;
        let quota = ValidatedTenantQuota::from_create_params(&params)?;
        let max_users = quota.max_users;
        let max_roles = quota.max_roles;
        let max_storage_mb = quota.max_storage_mb;
        let max_requests_per_minute = quota.max_requests_per_minute;
        let tenant_id = params.tenant_id.clone();
        // switch_token 持久化在 pending placement 中，作为控制库权威的幂等键与
        // 非敏感请求指纹；管理员密码另由已持久化的 Argon2 摘要强校验。
        let switch_token = provisioning_switch_token(
            &params,
            max_users,
            max_roles,
            max_storage_mb,
            max_requests_per_minute,
        );
        let pending = self
            .tenant_data
            .prepare_provisioning(
                tenant_id.clone(),
                params.data_target_key.clone(),
                1,
                switch_token,
            )
            .map_err(map_tenant_data_error)?;
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let existing = self
            .tenant_repo
            .lock_optional_tenant_in_txn(&transaction, &tenant_id)
            .await?;
        let already_enabled = if existing.is_some() {
            self.resume_provisioning_in_txn(&transaction, &params, &quota, &pending)
                .await?
        } else {
            let capability_resources = self
                .product_service
                .provisioning_resources_in_txn(&transaction, params.plan_version_id)
                .await?;
            let command = ProvisionTenantCommand {
                provisioning_request_token: pending.switch_token.clone(),
                tenant_id: tenant_id.clone(),
                name: params.name.clone(),
                domain: params.domain.clone(),
                expire_at: params.expire_at,
                max_users,
                max_roles,
                max_storage_mb,
                max_requests_per_minute,
                admin_username: params.admin_username.clone(),
                admin_password_hash: ryframe_auth::password::hash(&params.admin_password)?,
                // 首事务仅创建租户身份与非能力模板；Capability 资源必须等目标库
                // fence 成功后再同步，避免数据面尚未就绪时暴露模块入口。
                enabled_capability_route_keys: Vec::new(),
                enabled_capability_permission_codes: Vec::new(),
                managed_capability_route_keys: capability_resources.managed_route_keys,
                managed_capability_permission_codes: capability_resources.managed_permission_codes,
                default_admin_permission_codes: Vec::new(),
            };
            self.provisioning_repo
                .provision_in_transaction(&transaction, command)
                .await?;
            self.product_repo
                .assign_initial_in_txn(
                    &transaction,
                    &tenant_id,
                    params.plan_version_id,
                    actor.user_id,
                )
                .await?;
            TenantDataPlacementRepository
                .create_pending(&transaction, &pending)
                .await
                .map_err(map_tenant_data_error)?;
            false
        };
        crate::commit_current_audit(transaction).await?;
        if already_enabled {
            return self
                .tenant_repo
                .find_by_tenant_id(self.db.write(), &tenant_id)
                .await?
                .map(TenantVo::from)
                .ok_or_else(|| AppError::NotFound("租户不存在".into()));
        }

        if let Err(error) = self.tenant_data.provision_pending_fence(&pending).await {
            self.mark_provisioning_failed(&pending).await;
            return Err(map_tenant_data_error(error));
        }

        if let Err(error) = self
            .sync_provisioning_resources(&pending, params.plan_version_id)
            .await
        {
            self.mark_provisioning_failed(&pending).await;
            return Err(error);
        }

        let finalization = self.finalize_provisioning(&pending).await;
        if let Err(error) = finalization {
            self.mark_provisioning_failed(&pending).await;
            return Err(error);
        }
        let enabled = self
            .tenant_repo
            .find_by_tenant_id(self.db.write(), &tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        Ok(TenantVo::from(enabled))
    }

    async fn resume_provisioning_in_txn(
        &self,
        transaction: &sea_orm::DatabaseTransaction,
        params: &CreateTenantParams,
        quota: &ValidatedTenantQuota,
        pending: &PendingTenantDataPlacement,
    ) -> AppResult<bool> {
        let existing = self
            .tenant_repo
            .lock_tenant_in_txn(transaction, &params.tenant_id)
            .await?;
        let request = self
            .provisioning_repo
            .lock_provision_request_in_txn(transaction, &params.tenant_id)
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
        if existing.status == tenant::Model::STATUS_ENABLED {
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
            tenant::Model::STATUS_PROVISIONING
                | tenant::Model::STATUS_PROVISIONING_FAILED
                | tenant::Model::STATUS_ENABLED
        ) {
            return Err(AppError::Conflict("现有租户状态不允许恢复创建 Saga".into()));
        }
        let assignment = self
            .product_repo
            .assignment(transaction, &params.tenant_id)
            .await?
            .ok_or_else(|| AppError::Conflict("现有租户缺少 provisioning 套餐快照".into()))?;
        if assignment.plan_version_id != params.plan_version_id {
            return Err(AppError::Conflict(
                "租户创建重试的 plan_version_id 与已持久化请求不一致".into(),
            ));
        }
        let admin = self
            .provisioning_repo
            .find_user_by_username(transaction, &params.tenant_id, &params.admin_username)
            .await?
            .ok_or_else(|| AppError::Conflict("租户创建重试的管理员账号不匹配".into()))?;
        if !ryframe_auth::password::verify(&params.admin_password, &admin.password_hash)? {
            return Err(AppError::Conflict(
                "租户创建重试的管理员凭据与已持久化请求不一致".into(),
            ));
        }
        TenantDataPlacementRepository
            .create_or_resume_pending(transaction, pending)
            .await
            .map_err(map_tenant_data_error)?;
        if existing.status == tenant::Model::STATUS_PROVISIONING_FAILED {
            self.tenant_repo
                .update_status(
                    transaction,
                    &params.tenant_id,
                    tenant::Model::STATUS_PROVISIONING,
                )
                .await?;
        }
        Ok(false)
    }

    async fn sync_provisioning_resources(
        &self,
        pending: &PendingTenantDataPlacement,
        plan_version_id: i64,
    ) -> AppResult<()> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let tenant = self
            .tenant_repo
            .lock_tenant_in_txn(&transaction, &pending.tenant_id)
            .await?;
        if tenant.status != tenant::Model::STATUS_PROVISIONING {
            return Err(AppError::TenantOperationConflict(
                "租户已不处于 provisioning，不能同步初始化能力资源".into(),
            ));
        }
        self.product_service
            .sync_provisioning_resources_in_txn(&transaction, &pending.tenant_id, plan_version_id)
            .await?;
        crate::commit_current_audit(transaction).await
    }

    async fn finalize_provisioning(&self, pending: &PendingTenantDataPlacement) -> AppResult<()> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let tenant = self
            .tenant_repo
            .lock_tenant_in_txn(&transaction, &pending.tenant_id)
            .await?;
        if tenant.status == tenant::Model::STATUS_ENABLED {
            TenantDataPlacementRepository
                .activate(&transaction, pending)
                .await
                .map_err(map_tenant_data_error)?;
            return transaction.commit().await.map_err(database_error);
        }
        if tenant.status != tenant::Model::STATUS_PROVISIONING {
            return Err(AppError::Conflict(
                "租户 provisioning 状态已变化，无法完成启用".into(),
            ));
        }
        TenantDataPlacementRepository
            .activate(&transaction, pending)
            .await
            .map_err(map_tenant_data_error)?;
        self.tenant_repo
            .update_status(
                &transaction,
                &pending.tenant_id,
                tenant::Model::STATUS_ENABLED,
            )
            .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn mark_provisioning_failed(&self, pending: &PendingTenantDataPlacement) {
        let Ok(transaction) = self.db.write().begin().await else {
            tracing::error!(tenant_id = %pending.tenant_id, "无法开启租户 provisioning 失败补偿事务");
            return;
        };
        let result = async {
            let tenant = self
                .tenant_repo
                .lock_tenant_in_txn(&transaction, &pending.tenant_id)
                .await?;
            if tenant.status == tenant::Model::STATUS_ENABLED {
                return Ok(());
            }
            TenantDataPlacementRepository
                .fail(&transaction, pending)
                .await
                .map_err(map_tenant_data_error)?;
            self.tenant_repo
                .update_status(
                    &transaction,
                    &pending.tenant_id,
                    tenant::Model::STATUS_PROVISIONING_FAILED,
                )
                .await
        }
        .await;
        match result {
            Ok(()) => {
                if let Err(error) = transaction.commit().await {
                    tracing::error!(tenant_id = %pending.tenant_id, %error, "租户 provisioning 失败补偿提交失败");
                }
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                tracing::error!(tenant_id = %pending.tenant_id, %error, "租户 provisioning 失败补偿执行失败");
            }
        }
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
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let mut tenant = self
            .tenant_repo
            .lock_and_validate_resource_limits_in_txn(
                &transaction,
                tenant_id,
                params.max_users,
                params.max_roles,
                params.max_storage_mb,
            )
            .await?;
        if !matches!(
            tenant.status.as_str(),
            tenant::Model::STATUS_ENABLED | tenant::Model::STATUS_DISABLED
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

        let active: tenant::ActiveModel = tenant.into();
        let saved = active
            .reset_all()
            .update(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
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
            tenant::Model::STATUS_ENABLED | tenant::Model::STATUS_DISABLED
        ) {
            return Err(AppError::Validation(
                "租户状态只能切换为 enabled 或 disabled".into(),
            ));
        }
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let current = self
            .tenant_repo
            .lock_tenant_in_txn(&transaction, tenant_id)
            .await?;
        if !matches!(
            current.status.as_str(),
            tenant::Model::STATUS_ENABLED | tenant::Model::STATUS_DISABLED
        ) {
            return Err(AppError::Conflict(
                "provisioning 租户只能由创建 Saga 完成或重试，不能直接切换状态".into(),
            ));
        }
        if current.status == status {
            return crate::commit_current_audit(transaction).await;
        }
        self.tenant_repo
            .update_status(&transaction, tenant_id, &status)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await
    }
}

fn ensure_platform_admin(actor: &ActorContext) -> AppResult<()> {
    crate::validated_tenant_id(actor)?;
    if actor.tenant_id != SYSTEM_TENANT_ID {
        return Err(AppError::Authorization(
            "仅 system 租户中已获相应 RBAC 权限的操作员可以管理租户".into(),
        ));
    }
    Ok(())
}

fn validate_tenant_limits(
    max_users: i32,
    max_roles: i32,
    max_storage_mb: i64,
    max_requests_per_min: i32,
) -> AppResult<()> {
    if max_users != 0 && max_users < MIN_TENANT_USERS {
        return Err(AppError::Validation(format!(
            "用户额度不能低于 {MIN_TENANT_USERS}"
        )));
    }
    if max_roles != 0 && max_roles < MIN_TENANT_ROLES {
        return Err(AppError::Validation(format!(
            "角色额度不能低于 {MIN_TENANT_ROLES}"
        )));
    }
    if max_storage_mb < 0 {
        return Err(AppError::Validation("存储额度不能为负数".into()));
    }
    if max_requests_per_min < 0 {
        return Err(AppError::Validation("每分钟请求额度不能为负数".into()));
    }
    Ok(())
}

fn validate_data_target_key(value: &str) -> AppResult<()> {
    if value == value.trim() && crate::runtime_policy::is_valid_tenant_target_key(value) {
        Ok(())
    } else {
        Err(AppError::Validation(
            "data_target_key 必须为 2–64 位 ASCII 字母、数字、下划线或连字符，且首尾必须是字母或数字"
                .into(),
        ))
    }
}

fn validate_idempotency_key(value: &str) -> AppResult<()> {
    if (16..=128).contains(&value.len()) && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        Ok(())
    } else {
        Err(AppError::Validation(
            "Idempotency-Key 必须为 16 到 128 个可见 ASCII 字符".into(),
        ))
    }
}

fn provisioning_switch_token(
    params: &CreateTenantParams,
    max_users: i32,
    max_roles: i32,
    max_storage_mb: i64,
    max_requests_per_minute: i32,
) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(params.idempotency_key.as_bytes())
        .expect("HMAC accepts arbitrary Idempotency-Key lengths");
    mac.update(b"ryframe:tenant-provisioning:v3\0");
    update_fingerprint_field(&mut mac, params.tenant_id.as_bytes());
    update_fingerprint_field(&mut mac, params.name.as_bytes());
    update_optional_fingerprint_field(&mut mac, params.domain.as_deref());
    match params.expire_at {
        Some(value) => {
            mac.update(&[1]);
            mac.update(&value.timestamp_micros().to_be_bytes());
        }
        None => mac.update(&[0]),
    }
    mac.update(&max_users.to_be_bytes());
    mac.update(&max_roles.to_be_bytes());
    mac.update(&max_storage_mb.to_be_bytes());
    mac.update(&max_requests_per_minute.to_be_bytes());
    update_fingerprint_field(&mut mac, params.admin_username.as_bytes());
    mac.update(&params.plan_version_id.to_be_bytes());
    update_fingerprint_field(&mut mac, params.data_target_key.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn update_optional_fingerprint_field(mac: &mut Hmac<Sha256>, value: Option<&str>) {
    match value {
        Some(value) => {
            mac.update(&[1]);
            update_fingerprint_field(mac, value.as_bytes());
        }
        None => mac.update(&[0]),
    }
}

fn update_fingerprint_field(mac: &mut Hmac<Sha256>, value: &[u8]) {
    mac.update(&(value.len() as u64).to_be_bytes());
    mac.update(value);
}

fn map_tenant_data_error(error: TenantDataError) -> AppError {
    let message = error.to_string();
    match error {
        TenantDataError::InvalidConfiguration(_)
        | TenantDataError::InvalidTenantId(_)
        | TenantDataError::UnknownTarget { .. } => AppError::Validation(message),
        TenantDataError::PlacementUnavailable { .. }
        | TenantDataError::InvalidPlacement { .. }
        | TenantDataError::FenceRejected { .. }
        | TenantDataError::DedicatedTargetOccupied { .. } => {
            AppError::TenantOperationConflict(message)
        }
        TenantDataError::StalePlacementGeneration { .. } => {
            AppError::StalePlacementGeneration(message)
        }
        TenantDataError::TenantDataMaintenance { .. } => {
            AppError::TenantDataMaintenance(message, 5)
        }
        TenantDataError::TargetUnavailable { .. }
        | TenantDataError::PoolCapacityExhausted { .. }
        | TenantDataError::ConnectionBudgetExhausted { .. } => {
            AppError::TenantDataTargetUnavailable(message, 5)
        }
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
