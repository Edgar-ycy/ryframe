use std::collections::{BTreeSet, HashSet};

use ryframe_core::{
    Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::{DatabaseCluster, ReadConsistency};
use ryframe_db::{PermissionRepository, entities::permission};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::TransactionTrait;

use crate::AuthorizationCache;

mod model;
mod tree;

pub use model::{
    CreatePermissionCommand, PermissionSyncReport, PermissionTreeNode, PermissionType,
    PermissionVo, UpdatePermissionCommand,
};
pub use tree::build_perm_tree;

pub struct PermissionService {
    db: DatabaseCluster,
    perm_repo: PermissionRepository,
    authorization_cache: AuthorizationCache,
}

impl PermissionService {
    pub fn new(db: DatabaseCluster, authorization_cache: AuthorizationCache) -> Self {
        Self {
            db,
            perm_repo: PermissionRepository,
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
        let db = self.db.write();
        if self
            .perm_repo
            .find_by_code(db, tenant_id, &command.code)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("权限码已存在".into()));
        }
        let mut model = permission::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            name: command.name,
            code: command.code,
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
        let saved = self
            .perm_repo
            .insert_in_transaction(&transaction, tenant_id, model)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
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
        let db = self.db.write();
        let existing = self
            .perm_repo
            .find_by_id(db, tenant_id, command.id)
            .await?
            .ok_or_else(|| AppError::NotFound("权限不存在".into()))?;
        if existing.code != command.code
            && self
                .perm_repo
                .find_by_code(db, tenant_id, &command.code)
                .await?
                .is_some()
        {
            return Err(AppError::Conflict("权限码已存在".into()));
        }
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let mut model = self
            .perm_repo
            .find_by_id_for_update(&transaction, tenant_id, command.id)
            .await?
            .ok_or_else(|| AppError::NotFound("权限不存在".into()))?;
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
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await?;
        Ok(PermissionVo::from(saved))
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        if self.perm_repo.is_referenced(db, tenant_id, id).await? {
            return Err(AppError::Conflict(
                "权限仍被角色或菜单引用，不能删除".into(),
            ));
        }
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        self.perm_repo
            .find_by_id_for_update(&transaction, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("权限不存在".into()))?;
        self.perm_repo
            .delete_in_transaction(&transaction, tenant_id, id)
            .await?;
        let authorization_epoch = self
            .authorization_cache
            .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
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
            .collect::<BTreeSet<_>>();
        let existing = self.perm_repo.find_all(db, tenant_id).await?;
        let existing_codes: HashSet<String> = existing.iter().map(|p| p.code.clone()).collect();
        let scanned_total = scanned.len();
        let mut missing = Vec::new();
        let mut models = Vec::new();

        for code in scanned {
            if existing_codes.contains(&code) {
                continue;
            }
            missing.push(code.clone());
            let name = code.rsplit(':').next().unwrap_or(&code).to_string();
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
            models.push(model);
        }

        let created = models.len();
        if created > 0 {
            let transaction = db
                .begin()
                .await
                .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
            for model in models {
                self.perm_repo
                    .insert_in_transaction(&transaction, tenant_id, model)
                    .await?;
            }
            let authorization_epoch = self
                .authorization_cache
                .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
                .await?;
            crate::commit_current_audit(transaction).await?;
            self.authorization_cache
                .sync_tenant_epoch(tenant_id, authorization_epoch)
                .await?;
        }

        Ok(PermissionSyncReport {
            scanned: scanned_total,
            existing: existing_codes.len(),
            created,
            missing,
        })
    }
}
