use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Duration, Utc};
use ryframe_config::{PepperKeyring, ServiceAccountsConfig};
use ryframe_core::{PageResult, Repository, ValidatedPageQuery};
use ryframe_db::{
    DataRetentionRepository, DatabaseCluster, PermissionRepository, ReadConsistency,
    RoleRepository, ServiceAccountLock, ServiceAccountRepository, ServiceCredentialRepository,
    ServiceDelegationRepository, UserRepository,
    entities::{
        role, service_access_audit, service_account, service_account_role, service_credential,
        service_delegation, service_delegation_capability,
    },
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, TransactionTrait,
    sea_query::{Expr, LockType},
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    AuthorizationCache,
    service_identity_secret::{IssuedApiKey, IssuedDelegationToken},
};

const IDEMPOTENCY_DOMAIN: &[u8] = b"ryframe/idempotency-key/v1\0";
const FINGERPRINT_DOMAIN: &[u8] = b"ryframe/request-fingerprint/v1\0";

/// 可由 Agent 注册表提供给管理域的稳定能力描述。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ServiceCapabilityDescriptor {
    pub key: String,
    pub permission: String,
    pub direct: bool,
    pub delegated: bool,
}

#[derive(Clone, Debug)]
pub struct CreateServiceAccountCommand {
    pub code: String,
    pub name: String,
    pub description: Option<String>,
    pub dept_id: Option<i64>,
    pub max_requests_per_minute: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct UpdateServiceAccountCommand {
    pub name: String,
    pub description: Option<String>,
    pub dept_id: Option<i64>,
    pub max_requests_per_minute: i32,
}

#[derive(Clone, Debug)]
pub struct CreateCredentialCommand {
    pub label: String,
    pub expires_at: DateTime<Utc>,
    pub idempotency_key: String,
}

#[derive(Clone, Debug)]
pub struct CreateDelegationCommand {
    pub account_id: i64,
    pub capability_keys: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub reason: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceAccountVo {
    pub id: String,
    pub code: String,
    pub name: String,
    pub description: Option<String>,
    pub dept_id: Option<String>,
    pub status: String,
    pub authorization_version: i32,
    pub max_requests_per_minute: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<service_account::Model> for ServiceAccountVo {
    fn from(account: service_account::Model) -> Self {
        Self {
            id: account.id.to_string(),
            code: account.code,
            name: account.name,
            description: account.description,
            dept_id: account.dept_id.map(|id| id.to_string()),
            status: account.status,
            authorization_version: account.authorization_version,
            max_requests_per_minute: account.max_requests_per_minute,
            created_at: account.created_at,
            updated_at: account.updated_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceAccountDetailVo {
    #[serde(flatten)]
    pub account: ServiceAccountVo,
    pub role_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceCredentialVo {
    pub id: String,
    pub account_id: String,
    pub key_id: String,
    pub label: String,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<service_credential::Model> for ServiceCredentialVo {
    fn from(credential: service_credential::Model) -> Self {
        Self {
            id: credential.id.to_string(),
            account_id: credential.account_id.to_string(),
            key_id: format!("rfk_{}", credential.key_id),
            label: credential.label,
            status: credential.status,
            expires_at: credential.expires_at,
            last_used_at: credential.last_used_at,
            revoked_at: credential.revoked_at,
            created_at: credential.created_at,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct CreatedCredentialVo {
    #[serde(flatten)]
    pub credential: ServiceCredentialVo,
    /// 只在首次成功创建时返回；幂等重放永远为 `None`。
    pub secret: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceDelegationVo {
    pub id: String,
    pub account_id: String,
    pub user_id: String,
    pub status: String,
    pub version: i32,
    pub not_before: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub reason: String,
    pub capability_keys: Vec<String>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceDelegationTargetVo {
    pub account_id: String,
    pub code: String,
    pub name: String,
    pub capabilities: Vec<ServiceCapabilityDescriptor>,
}

#[derive(Clone, Serialize)]
pub struct CreatedDelegationVo {
    #[serde(flatten)]
    pub delegation: ServiceDelegationVo,
    /// 只在首次成功创建时返回；幂等重放永远为 `None`。
    pub token: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceAccessAuditVo {
    pub id: String,
    pub request_id: String,
    pub tenant_id: Option<String>,
    pub account_id: Option<String>,
    pub credential_id: Option<String>,
    pub delegation_id: Option<String>,
    pub represented_user_id: Option<String>,
    pub operation_id: String,
    pub capability_key: String,
    pub required_permission: String,
    pub access_mode: String,
    pub result: String,
    pub reason_code: String,
    pub http_status: i32,
    pub row_count: Option<i32>,
    pub response_bytes: Option<i64>,
    pub tenant_epoch: Option<i32>,
    pub account_authorization_version: Option<i32>,
    pub user_authorization_version: Option<i32>,
    pub delegation_version: Option<i32>,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

impl From<service_access_audit::Model> for ServiceAccessAuditVo {
    fn from(audit: service_access_audit::Model) -> Self {
        Self {
            id: audit.id.to_string(),
            request_id: audit.request_id,
            tenant_id: audit.tenant_id,
            account_id: audit.account_id.map(|id| id.to_string()),
            credential_id: audit.credential_id.map(|id| id.to_string()),
            delegation_id: audit.delegation_id.map(|id| id.to_string()),
            represented_user_id: audit.represented_user_id.map(|id| id.to_string()),
            operation_id: audit.operation_id,
            capability_key: audit.capability_key,
            required_permission: audit.required_permission,
            access_mode: audit.access_mode,
            result: audit.result,
            reason_code: audit.reason_code,
            http_status: audit.http_status,
            row_count: audit.row_count,
            response_bytes: audit.response_bytes,
            tenant_epoch: audit.tenant_epoch,
            account_authorization_version: audit.account_authorization_version,
            user_authorization_version: audit.user_authorization_version,
            delegation_version: audit.delegation_version,
            started_at: audit.started_at,
            completed_at: audit.completed_at,
        }
    }
}

pub struct ServiceAccountService {
    db: DatabaseCluster,
    config: ServiceAccountsConfig,
    keyring: Arc<PepperKeyring>,
    capabilities: Vec<ServiceCapabilityDescriptor>,
    account_repo: ServiceAccountRepository,
    credential_repo: ServiceCredentialRepository,
    delegation_repo: ServiceDelegationRepository,
    role_repo: RoleRepository,
    permission_repo: PermissionRepository,
    user_repo: UserRepository,
    authorization_cache: AuthorizationCache,
}

impl ServiceAccountService {
    pub fn new(
        db: DatabaseCluster,
        config: ServiceAccountsConfig,
        keyring: Arc<PepperKeyring>,
        capabilities: Vec<ServiceCapabilityDescriptor>,
        authorization_cache: AuthorizationCache,
    ) -> AppResult<Self> {
        config.validate().map_err(AppError::Config)?;
        validate_capabilities(&capabilities)?;
        Ok(Self {
            db,
            config,
            keyring,
            capabilities,
            account_repo: ServiceAccountRepository,
            credential_repo: ServiceCredentialRepository,
            delegation_repo: ServiceDelegationRepository,
            role_repo: RoleRepository,
            permission_repo: PermissionRepository,
            user_repo: UserRepository,
            authorization_cache,
        })
    }

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

    pub async fn account_role_ids(
        &self,
        actor: &ActorContext,
        account_id: i64,
    ) -> AppResult<Vec<String>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        self.account_repo
            .find_by_id(&db, tenant_id, account_id)
            .await?
            .filter(service_account::Model::is_enabled)
            .ok_or_else(|| AppError::NotFound("可用的服务账号不存在".into()))?;
        Ok(self
            .account_repo
            .role_ids(&db, tenant_id, account_id)
            .await?
            .into_iter()
            .map(|id| id.to_string())
            .collect())
    }

    pub async fn replace_account_roles(
        &self,
        actor: &ActorContext,
        account_id: i64,
        mut role_ids: Vec<i64>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        role_ids.sort_unstable();
        role_ids.dedup();
        let txn = self.db.write().begin().await.map_err(database_error)?;
        self.account_repo
            .lock_tenant_in_txn(&txn, tenant_id, ServiceAccountLock::Update)
            .await?;
        self.lock_account(&txn, tenant_id, account_id).await?;
        self.account_repo
            .replace_roles_in_txn(&txn, tenant_id, account_id, &role_ids)
            .await?;
        self.account_repo
            .increment_authorization_version_in_txn(&txn, tenant_id, account_id)
            .await?;
        let epoch = self.bump_tenant_epoch(&txn, tenant_id).await?;
        crate::commit_current_audit(txn).await?;
        self.sync_committed_authorization_state(tenant_id, epoch, &[])
            .await;
        Ok(())
    }

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
            id: snowflake::try_next_snowflake_id()?,
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
            id: snowflake::try_next_snowflake_id()?,
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

    pub async fn list_access_audits(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<ServiceAccessAuditVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let base = service_access_audit::Entity::find()
            .filter(service_access_audit::Column::TenantId.eq(tenant_id));
        let total = base.clone().count(&db).await.map_err(database_error)?;
        let rows = base
            .order_by_desc(service_access_audit::Column::StartedAt)
            .offset(page.offset())
            .limit(page.page_size())
            .all(&db)
            .await
            .map_err(database_error)?;
        Ok(PageResult::new(
            rows.into_iter().map(ServiceAccessAuditVo::from).collect(),
            total,
            &page,
        ))
    }

    async fn revoke_delegation(
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

    async fn lock_account(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
    ) -> AppResult<service_account::Model> {
        self.account_repo
            .find_by_id_in_txn(txn, tenant_id, account_id, ServiceAccountLock::Update)
            .await?
            .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))
    }

    async fn bump_tenant_epoch(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<i32> {
        self.authorization_cache
            .increment_tenant_epoch_in_transaction(txn, tenant_id)
            .await
    }

    /// 数据库事实与授权镜像修复 Outbox 已在同一事务提交；即时同步只用于降低传播延迟。
    /// 提交后 Redis 故障不能把已经生效的写操作伪装成失败，尤其不能丢失只显示一次的 Secret。
    async fn sync_committed_authorization_state(
        &self,
        tenant_id: &str,
        authorization_epoch: i32,
        user_versions: &[(i64, i32)],
    ) {
        if !user_versions.is_empty()
            && let Err(error) = self
                .authorization_cache
                .sync_user_versions(tenant_id, user_versions)
                .await
        {
            tracing::warn!(
                tenant_id,
                %error,
                "服务账号写入已提交，用户授权镜像将由 Outbox 修复"
            );
        }
        if let Err(error) = self
            .authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await
        {
            tracing::warn!(
                tenant_id,
                %error,
                "服务账号写入已提交，租户授权镜像将由 Outbox 修复"
            );
        }
    }

    async fn user_permission_codes(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<HashSet<String>> {
        let roles = self
            .role_repo
            .find_user_roles(db, tenant_id, user_id)
            .await?;
        let ids = roles.into_iter().map(|role| role.id).collect::<Vec<_>>();
        Ok(self
            .permission_repo
            .find_role_perms(db, tenant_id, &ids)
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect())
    }

    async fn account_permission_codes(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        account_id: i64,
    ) -> AppResult<HashSet<String>> {
        let role_ids = self
            .account_repo
            .role_ids(db, tenant_id, account_id)
            .await?;
        let enabled_role_ids = if role_ids.is_empty() {
            Vec::new()
        } else {
            ryframe_db::entities::role::Entity::find()
                .filter(ryframe_db::entities::role::Column::TenantId.eq(tenant_id))
                .filter(ryframe_db::entities::role::Column::Id.is_in(role_ids))
                .filter(
                    ryframe_db::entities::role::Column::Status
                        .eq(ryframe_db::entities::role::Model::STATUS_NORMAL),
                )
                .filter(
                    ryframe_db::entities::role::Column::DelFlag
                        .eq(ryframe_db::entities::role::Model::DEL_FLAG_NORMAL),
                )
                .all(db)
                .await
                .map_err(database_error)?
                .into_iter()
                .map(|role| role.id)
                .collect()
        };
        Ok(self
            .permission_repo
            .find_role_perms(db, tenant_id, &enabled_role_ids)
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect())
    }

    async fn common_capability_keys_in_txn(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
        user_id: i64,
    ) -> AppResult<HashSet<String>> {
        let user_roles = self
            .role_repo
            .find_user_roles_all_status(txn, tenant_id, user_id)
            .await?
            .into_iter()
            .filter(|role| role.status == role::Model::STATUS_NORMAL)
            .map(|role| role.id)
            .collect::<Vec<_>>();
        let account_role_ids = self
            .account_repo
            .role_ids(txn, tenant_id, account_id)
            .await?;
        let account_roles = if account_role_ids.is_empty() {
            Vec::new()
        } else {
            ryframe_db::entities::role::Entity::find()
                .filter(ryframe_db::entities::role::Column::TenantId.eq(tenant_id))
                .filter(
                    ryframe_db::entities::role::Column::Id.is_in(account_role_ids.iter().copied()),
                )
                .filter(
                    ryframe_db::entities::role::Column::Status
                        .eq(ryframe_db::entities::role::Model::STATUS_NORMAL),
                )
                .filter(
                    ryframe_db::entities::role::Column::DelFlag
                        .eq(ryframe_db::entities::role::Model::DEL_FLAG_NORMAL),
                )
                .all(txn)
                .await
                .map_err(database_error)?
                .into_iter()
                .map(|role| role.id)
                .collect()
        };
        let user_permissions = permission_codes_in_txn(txn, tenant_id, &user_roles).await?;
        let account_permissions = permission_codes_in_txn(txn, tenant_id, &account_roles).await?;
        Ok(
            common_capabilities(&self.capabilities, &user_permissions, &account_permissions)
                .into_iter()
                .map(|capability| capability.key)
                .collect(),
        )
    }

    async fn delegation_vo<C>(
        &self,
        db: &C,
        delegation: service_delegation::Model,
    ) -> AppResult<ServiceDelegationVo>
    where
        C: sea_orm::ConnectionTrait,
    {
        let keys = self
            .delegation_repo
            .capability_keys(db, &delegation.tenant_id, delegation.id)
            .await?;
        Ok(delegation_vo_with_keys(delegation, keys))
    }

    async fn delegations_with_capabilities(
        &self,
        db: &DatabaseConnection,
        rows: Vec<service_delegation::Model>,
    ) -> AppResult<Vec<ServiceDelegationVo>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let delegation_ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
        let capabilities = service_delegation_capability::Entity::find()
            .filter(service_delegation_capability::Column::TenantId.eq(&rows[0].tenant_id))
            .filter(
                service_delegation_capability::Column::DelegationId
                    .is_in(delegation_ids.iter().copied()),
            )
            .order_by_asc(service_delegation_capability::Column::DelegationId)
            .order_by_asc(service_delegation_capability::Column::CapabilityKey)
            .all(db)
            .await
            .map_err(database_error)?;
        let mut by_delegation: HashMap<i64, Vec<String>> = HashMap::new();
        for capability in capabilities {
            by_delegation
                .entry(capability.delegation_id)
                .or_default()
                .push(capability.capability_key);
        }
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let keys = by_delegation.remove(&row.id).unwrap_or_default();
            result.push(delegation_vo_with_keys(row, keys));
        }
        Ok(result)
    }

    fn ensure_enabled(&self) -> AppResult<()> {
        if self.config.enabled {
            Ok(())
        } else {
            Err(AppError::ServiceUnavailable("服务账号功能未启用".into()))
        }
    }
}

fn validate_capabilities(capabilities: &[ServiceCapabilityDescriptor]) -> AppResult<()> {
    let mut keys = BTreeSet::new();
    for capability in capabilities {
        if capability.key.trim().is_empty() || capability.permission.trim().is_empty() {
            return Err(AppError::Config(
                "服务能力 key 和 permission 不能为空".into(),
            ));
        }
        if !capability.direct && !capability.delegated {
            return Err(AppError::Config(format!(
                "服务能力 {} 未启用任何访问模式",
                capability.key
            )));
        }
        if !keys.insert(capability.key.as_str()) {
            return Err(AppError::Config(format!(
                "服务能力 {} 重复",
                capability.key
            )));
        }
    }
    Ok(())
}

fn common_capabilities(
    capabilities: &[ServiceCapabilityDescriptor],
    user_permissions: &HashSet<String>,
    account_permissions: &HashSet<String>,
) -> Vec<ServiceCapabilityDescriptor> {
    let user_permissions = user_permissions.iter().cloned().collect::<Vec<_>>();
    let account_permissions = account_permissions.iter().cloned().collect::<Vec<_>>();
    capabilities
        .iter()
        .filter(|capability| {
            capability.delegated
                && ryframe_auth::rbac::has_permission(
                    &user_permissions,
                    capability.permission.as_str(),
                )
                && ryframe_auth::rbac::has_permission(
                    &account_permissions,
                    capability.permission.as_str(),
                )
        })
        .cloned()
        .collect()
}

async fn account_permission_codes_for_accounts(
    db: &DatabaseConnection,
    tenant_id: &str,
    account_ids: &[i64],
) -> AppResult<HashMap<i64, HashSet<String>>> {
    if account_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let account_roles = service_account_role::Entity::find()
        .filter(service_account_role::Column::TenantId.eq(tenant_id))
        .filter(service_account_role::Column::AccountId.is_in(account_ids.iter().copied()))
        .all(db)
        .await
        .map_err(database_error)?;
    if account_roles.is_empty() {
        return Ok(HashMap::new());
    }
    let role_ids = account_roles
        .iter()
        .map(|relation| relation.role_id)
        .collect::<HashSet<_>>();
    let enabled_role_ids = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::Id.is_in(role_ids.iter().copied()))
        .filter(role::Column::Status.eq(role::Model::STATUS_NORMAL))
        .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
        .all(db)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|role| role.id)
        .collect::<HashSet<_>>();
    if enabled_role_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let role_permissions = ryframe_db::entities::role_permission::Entity::find()
        .filter(ryframe_db::entities::role_permission::Column::TenantId.eq(tenant_id))
        .filter(
            ryframe_db::entities::role_permission::Column::RoleId
                .is_in(enabled_role_ids.iter().copied()),
        )
        .all(db)
        .await
        .map_err(database_error)?;
    let permission_ids = role_permissions
        .iter()
        .map(|relation| relation.perm_id)
        .collect::<HashSet<_>>();
    let permission_codes = if permission_ids.is_empty() {
        HashMap::new()
    } else {
        ryframe_db::entities::permission::Entity::find()
            .filter(ryframe_db::entities::permission::Column::TenantId.eq(tenant_id))
            .filter(
                ryframe_db::entities::permission::Column::Id.is_in(permission_ids.iter().copied()),
            )
            .filter(ryframe_db::entities::permission::Column::Status.eq("1"))
            .all(db)
            .await
            .map_err(database_error)?
            .into_iter()
            .map(|permission| (permission.id, permission.code))
            .collect::<HashMap<_, _>>()
    };
    let role_to_permissions = role_permissions.into_iter().fold(
        HashMap::<i64, HashSet<String>>::new(),
        |mut mapping, relation| {
            if let Some(code) = permission_codes.get(&relation.perm_id) {
                mapping
                    .entry(relation.role_id)
                    .or_default()
                    .insert(code.clone());
            }
            mapping
        },
    );
    let mut result = HashMap::<i64, HashSet<String>>::new();
    for relation in account_roles {
        if !enabled_role_ids.contains(&relation.role_id) {
            continue;
        }
        if let Some(codes) = role_to_permissions.get(&relation.role_id) {
            result
                .entry(relation.account_id)
                .or_default()
                .extend(codes.iter().cloned());
        }
    }
    Ok(result)
}

async fn validate_dept(
    db: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    dept_id: Option<i64>,
) -> AppResult<()> {
    let Some(dept_id) = dept_id else {
        return Ok(());
    };
    if ryframe_db::entities::dept::Entity::find_by_id(dept_id)
        .filter(ryframe_db::entities::dept::Column::TenantId.eq(tenant_id))
        .filter(
            ryframe_db::entities::dept::Column::DelFlag
                .eq(ryframe_db::entities::dept::Model::DEL_FLAG_NORMAL),
        )
        .lock(LockType::Share)
        .one(db)
        .await
        .map_err(database_error)?
        .is_none()
    {
        return Err(AppError::Validation("部门不存在或不属于当前租户".into()));
    }
    Ok(())
}

async fn permission_codes_in_txn(
    db: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    role_ids: &[i64],
) -> AppResult<HashSet<String>> {
    if role_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let permission_ids = ryframe_db::entities::role_permission::Entity::find()
        .filter(ryframe_db::entities::role_permission::Column::TenantId.eq(tenant_id))
        .filter(
            ryframe_db::entities::role_permission::Column::RoleId.is_in(role_ids.iter().copied()),
        )
        .all(db)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| row.perm_id)
        .collect::<Vec<_>>();
    if permission_ids.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(ryframe_db::entities::permission::Entity::find()
        .filter(ryframe_db::entities::permission::Column::TenantId.eq(tenant_id))
        .filter(ryframe_db::entities::permission::Column::Id.is_in(permission_ids))
        .filter(ryframe_db::entities::permission::Column::Status.eq("1"))
        .all(db)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|permission| permission.code)
        .collect())
}

fn delegation_vo_with_keys(
    delegation: service_delegation::Model,
    capability_keys: Vec<String>,
) -> ServiceDelegationVo {
    ServiceDelegationVo {
        id: delegation.id.to_string(),
        account_id: delegation.account_id.to_string(),
        user_id: delegation.user_id.to_string(),
        status: delegation.status,
        version: delegation.version,
        not_before: delegation.not_before,
        expires_at: delegation.expires_at,
        reason: delegation.reason,
        capability_keys,
        revoked_at: delegation.revoked_at,
        created_at: delegation.created_at,
    }
}

fn validate_code(value: String) -> AppResult<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(AppError::Validation(
            "服务账号代码只能包含小写字母、数字、连字符和下划线，且最长 64 字符".into(),
        ));
    }
    Ok(value)
}

fn required_text(value: String, field: &str, max: usize) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max {
        return Err(AppError::Validation(format!(
            "{field}不能为空且不能超过 {max} 个字符"
        )));
    }
    Ok(value.to_owned())
}

fn optional_text(value: Option<String>, max: usize) -> AppResult<Option<String>> {
    value
        .map(|value| required_text(value, "说明", max))
        .transpose()
}

fn validate_rate_limit(value: i32) -> AppResult<()> {
    if !(1..=10_000).contains(&value) {
        return Err(AppError::Validation(
            "每分钟请求上限必须在 1 到 10000 之间".into(),
        ));
    }
    Ok(())
}

fn validate_idempotency_key(value: String) -> AppResult<String> {
    let value = value.trim();
    if value.len() < 16 || value.len() > 128 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(AppError::Validation(
            "Idempotency-Key 必须为 16 到 128 个可见 ASCII 字符".into(),
        ));
    }
    Ok(value.to_owned())
}

fn request_fingerprint(parts: &[&[u8]]) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(FINGERPRINT_DOMAIN);
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    digest.finalize().to_vec()
}

fn unkeyed_hash(domain: &[u8], value: &[u8]) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(value);
    digest.finalize().to_vec()
}

fn ensure_same_fingerprint(existing: &[u8], requested: &[u8]) -> AppResult<()> {
    if existing == requested {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "相同 Idempotency-Key 已用于不同请求".into(),
        ))
    }
}

async fn database_now<C>(db: &C) -> AppResult<DateTime<Utc>>
where
    C: sea_orm::ConnectionTrait,
{
    DataRetentionRepository.database_utc_now(db).await
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
