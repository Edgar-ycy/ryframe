use chrono::{DateTime, Utc};
use ryframe_common::{ActorContext, AppError, AppResult};
use ryframe_db::DatabaseCluster;
use ryframe_db::{
    ProvisionTenantCommand, TenantProvisioningRepository, TenantRepository, entities::tenant,
};
use serde::Serialize;
use utoipa::ToSchema;

const SYSTEM_TENANT_ID: &str = "system";

#[derive(Debug, Serialize, ToSchema)]
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

#[derive(Debug, Clone)]
pub struct CreateTenantParams {
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
    db: DatabaseCluster,
    tenant_repo: TenantRepository,
    provisioning_repo: TenantProvisioningRepository,
}

impl TenantService {
    pub fn new(db: DatabaseCluster) -> Self {
        Self {
            db,
            tenant_repo: TenantRepository,
            provisioning_repo: TenantProvisioningRepository,
        }
    }

    pub async fn list(&self, actor: &ActorContext) -> AppResult<Vec<TenantVo>> {
        ensure_platform_admin(actor)?;
        self.tenant_repo
            .list_all(self.db.read())
            .await
            .map(|tenants| tenants.into_iter().map(TenantVo::from).collect())
    }

    pub async fn create(
        &self,
        actor: &ActorContext,
        params: CreateTenantParams,
    ) -> AppResult<TenantVo> {
        ensure_platform_admin(actor)?;
        ryframe_auth::password::validate_complexity(&params.admin_password)?;
        if self
            .tenant_repo
            .find_by_tenant_id(self.db.write(), &params.tenant_id)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("租户标识已存在".into()));
        }
        if self
            .provisioning_repo
            .admin_username_exists(self.db.write(), &params.tenant_id, &params.admin_username)
            .await?
        {
            return Err(AppError::Conflict("租户管理员用户名已存在".into()));
        }

        let command = ProvisionTenantCommand {
            tenant_id: params.tenant_id,
            name: params.name,
            domain: params.domain,
            expire_at: params.expire_at,
            max_users: params.max_users.unwrap_or(100),
            max_roles: params.max_roles.unwrap_or(20),
            max_storage_mb: params.max_storage_mb.unwrap_or(1024),
            max_requests_per_minute: params.max_requests_per_min.unwrap_or(1000),
            admin_username: params.admin_username,
            admin_password_hash: ryframe_auth::password::hash(&params.admin_password)?,
        };
        self.provisioning_repo
            .provision(self.db.write(), command)
            .await
            .map(TenantVo::from)
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        params: UpdateTenantParams,
    ) -> AppResult<TenantVo> {
        ensure_platform_admin(actor)?;
        let mut tenant = self
            .tenant_repo
            .find_by_tenant_id(self.db.write(), tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
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

        self.tenant_repo
            .update(self.db.write(), tenant)
            .await
            .map(TenantVo::from)
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
        if !matches!(status.as_str(), "0" | "1") {
            return Err(AppError::Validation("无效的租户状态".into()));
        }
        self.tenant_repo
            .update_status(self.db.write(), tenant_id, &status)
            .await
    }
}

fn ensure_platform_admin(actor: &ActorContext) -> AppResult<()> {
    crate::validated_tenant_id(actor)?;
    if actor.tenant_id != SYSTEM_TENANT_ID || !actor.is_super_admin {
        return Err(AppError::Authorization("仅平台管理员可以管理租户".into()));
    }
    Ok(())
}
