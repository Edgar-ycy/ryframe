use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use ryframe_adapters::{
    Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::{ControlDatabaseCluster, ReadConsistency};
use ryframe_db::{PermissionRepository, TenantConfigTransferRepository, entities::permission};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait};

use super::ProductService;
use crate::AuthorizationCache;

mod model;
mod tree;

pub use model::{
    CreatePermissionCommand, PermissionSyncReport, PermissionTreeNode, PermissionType,
    PermissionVo, UpdatePermissionCommand,
};
pub use tree::build_perm_tree;

const SYSTEM_TENANT_ID: &str = "system";
const TENANT_PERMISSION_PREFIX: &str = "tenant:";
const PLATFORM_PERMISSION_PREFIX: &str = "platform:";

pub struct PermissionService {
    db: ControlDatabaseCluster,
    perm_repo: PermissionRepository,
    product_service: Arc<ProductService>,
    authorization_cache: AuthorizationCache,
}

impl PermissionService {
    pub fn new(
        db: ControlDatabaseCluster,
        authorization_cache: AuthorizationCache,
        product_service: Arc<ProductService>,
    ) -> Self {
        Self {
            db,
            perm_repo: PermissionRepository,
            product_service,
            authorization_cache,
        }
    }

    pub async fn find_role_permission_codes(
        &self,
        actor: &ActorContext,
        role_ids: &[i64],
    ) -> AppResult<Vec<String>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        self.perm_repo
            .find_role_perms(&db, tenant_id, role_ids)
            .await
            .map(|permissions| {
                permissions
                    .into_iter()
                    .map(|permission| permission.code)
                    .collect()
            })
    }

    pub async fn find_role_permission_ids(
        &self,
        actor: &ActorContext,
        role_id: i64,
    ) -> AppResult<Vec<i64>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        self.perm_repo
            .find_role_perm_ids(&db, tenant_id, role_id)
            .await
    }

    pub async fn list_all_perms(
        &self,
        actor: &ActorContext,
        perm_type: Option<&str>,
    ) -> AppResult<Vec<PermissionTreeNode>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let all = self.perm_repo.find_all(&db, tenant_id).await?;
        let filtered: Vec<&permission::Model> = if let Some(t) = perm_type {
            all.iter().filter(|p| p.perm_type == t).collect()
        } else {
            all.iter().collect()
        };

        let models: Vec<&permission::Model> = filtered;
        Ok(build_perm_tree(&models, None))
    }

    pub async fn find_by_id(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<Option<PermissionVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        self.perm_repo
            .find_by_id(&db, tenant_id, id)
            .await
            .map(|permission| permission.map(PermissionVo::from))
    }

    pub async fn create(
        &self,
        actor: &ActorContext,
        command: CreatePermissionCommand,
    ) -> AppResult<PermissionVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        ensure_tenant_permission_code_boundary(tenant_id, &command.code)?;
        let db = self.db.write();
        let mut model = permission::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            name: command.name,
            code: command.code.clone(),
            parent_id: command.parent_id,
            perm_type: command.perm_type.as_str().to_owned(),
            icon: command.icon,
            sort: command.sort,
            status: command.status,
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        model.fill_on_insert(&FillContext::new())?;
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        if permission::Entity::find()
            .filter(permission::Column::TenantId.eq(tenant_id))
            .filter(permission::Column::Code.eq(&command.code))
            .lock(sea_orm::sea_query::LockType::Update)
            .one(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .is_some()
        {
            return Err(AppError::Conflict("权限码已存在".into()));
        }
        if let Some(parent_id) = command.parent_id {
            permission::Entity::find_by_id(parent_id)
                .filter(permission::Column::TenantId.eq(tenant_id))
                .lock(sea_orm::sea_query::LockType::Update)
                .one(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?
                .ok_or_else(|| AppError::Validation("父权限不存在".into()))?;
        }
        let saved = self
            .perm_repo
            .insert_in_transaction(&transaction, tenant_id, model)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(PermissionVo::from(saved))
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        command: UpdatePermissionCommand,
    ) -> AppResult<PermissionVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        ensure_tenant_permission_code_boundary(tenant_id, &command.code)?;
        let db = self.db.write();
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        let mut model = self
            .perm_repo
            .find_by_id_for_update(&transaction, tenant_id, command.id)
            .await?
            .ok_or_else(|| AppError::NotFound("权限不存在".into()))?;
        if model.code != command.code
            && permission::Entity::find()
                .filter(permission::Column::TenantId.eq(tenant_id))
                .filter(permission::Column::Code.eq(&command.code))
                .lock(sea_orm::sea_query::LockType::Update)
                .one(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?
                .is_some()
        {
            return Err(AppError::Conflict("权限码已存在".into()));
        }
        if command.parent_id == Some(command.id) {
            return Err(AppError::Validation("权限不能将自己设为上级".into()));
        }
        let mut cursor = command.parent_id;
        while let Some(parent_id) = cursor {
            let parent = permission::Entity::find_by_id(parent_id)
                .filter(permission::Column::TenantId.eq(tenant_id))
                .lock(sea_orm::sea_query::LockType::Update)
                .one(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?
                .ok_or_else(|| AppError::Validation("父权限不存在".into()))?;
            if parent.id == command.id {
                return Err(AppError::Validation(
                    "不能将权限移动到自己的后代节点".into(),
                ));
            }
            cursor = parent.parent_id;
        }
        model.name = command.name;
        model.code = command.code;
        model.parent_id = command.parent_id;
        model.perm_type = command.perm_type.as_str().to_owned();
        model.icon = command.icon;
        model.sort = command.sort;
        model.status = command.status;
        model.fill_on_update(&FillContext::new())?;
        let saved = self
            .perm_repo
            .update_in_transaction(&transaction, tenant_id, model)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(PermissionVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        self.perm_repo
            .find_by_id_for_update(&transaction, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("权限不存在".into()))?;
        if self
            .perm_repo
            .is_referenced(&transaction, tenant_id, id)
            .await?
        {
            return Err(AppError::Conflict(
                "权限仍被角色或菜单引用，不能删除".into(),
            ));
        }
        self.perm_repo
            .delete_in_transaction(&transaction, tenant_id, id)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
            .await?;
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&transaction, tenant_id)
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(())
    }

    pub async fn sync_route_permissions(
        &self,
        actor: &ActorContext,
        route_permission_codes: &[&str],
    ) -> AppResult<PermissionSyncReport> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let scanned = route_permission_codes
            .iter()
            .map(|code| (*code).to_owned())
            // 编译期目录包含平台租户接口；普通租户同步时必须安全忽略，不能复制或因其存在而失败。
            .filter(|code| {
                tenant_id == SYSTEM_TENANT_ID
                    || (!is_tenant_permission_code(code.as_str())
                        && !is_platform_permission_code(code.as_str()))
            })
            .collect::<BTreeSet<_>>();
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        let scanned = self
            .product_service
            .filter_syncable_permission_codes_in_txn(&transaction, tenant_id, scanned)
            .await?;
        let scanned_total = scanned.len();
        let existing = permission::Entity::find()
            .filter(permission::Column::TenantId.eq(tenant_id))
            .lock(sea_orm::sea_query::LockType::Update)
            .all(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let existing_codes: HashSet<String> = existing.iter().map(|p| p.code.clone()).collect();
        let mut missing = Vec::new();
        for code in scanned {
            if !existing_codes.contains(&code) {
                missing.push(code);
            }
        }
        let created = missing.len();
        if created > 0 {
            for code in &missing {
                let name = code.rsplit(':').next().unwrap_or(code).to_string();
                let mut model = permission::Model {
                    id: snowflake::try_next_snowflake_id()?,
                    tenant_id: tenant_id.to_owned(),
                    name,
                    code: code.clone(),
                    parent_id: None,
                    perm_type: PermissionType::Api.as_str().to_owned(),
                    icon: None,
                    sort: 0,
                    status: "1".to_string(),
                    created_at: Default::default(),
                    updated_at: Default::default(),
                };
                model.fill_on_insert(&FillContext::new())?;
                self.perm_repo
                    .insert_in_transaction(&transaction, tenant_id, model)
                    .await?;
            }
            let authorization_epoch = self
                .authorization_cache
                .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
                .await?;
            TenantConfigTransferRepository
                .increment_configuration_version_in_txn(&transaction, tenant_id)
                .await?;
            crate::commit_current_audit(transaction).await?;
            self.authorization_cache
                .sync_tenant_epoch(tenant_id, authorization_epoch)
                .await?;
        } else {
            transaction
                .rollback()
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        }

        Ok(PermissionSyncReport {
            scanned: scanned_total,
            existing: existing_codes.len(),
            created,
            missing,
        })
    }
}

fn ensure_tenant_permission_code_boundary(tenant_id: &str, code: &str) -> AppResult<()> {
    if tenant_id != SYSTEM_TENANT_ID && is_tenant_permission_code(code) {
        return Err(AppError::Authorization(
            "租户管理权限仅允许系统租户维护".into(),
        ));
    }
    Ok(())
}

fn is_tenant_permission_code(code: &str) -> bool {
    code.get(..TENANT_PERMISSION_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(TENANT_PERMISSION_PREFIX))
}

fn is_platform_permission_code(code: &str) -> bool {
    code.get(..PLATFORM_PERMISSION_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PLATFORM_PERMISSION_PREFIX))
}
