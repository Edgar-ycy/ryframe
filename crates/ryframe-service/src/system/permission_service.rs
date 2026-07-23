use std::collections::{BTreeSet, HashSet};

use ryframe_common::{ActorContext, AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo, RedisClient, Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::DatabaseCluster;
use ryframe_db::{PermissionRepository, entities::permission};

mod model;
mod tree;

pub use model::{
    CreatePermissionCommand, PermissionSyncReport, PermissionTreeNode, PermissionType,
    PermissionVo, UpdatePermissionCommand,
};
pub use tree::build_perm_tree;

pub struct PermissionService {
    db: DatabaseCluster,
    perm_repo: LoggedRepo<PermissionRepository>,
}

impl PermissionService {
    pub fn new(db: DatabaseCluster, _redis: Option<RedisClient>) -> Self {
        Self {
            db,
            perm_repo: LoggedRepo::new(PermissionRepository),
        }
    }

    pub async fn find_role_permission_codes(
        &self,
        actor: &ActorContext,
        role_ids: &[i64],
    ) -> AppResult<Vec<String>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        self.perm_repo
            .find_role_perms(db, tenant_id, role_ids)
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
        let db = self.db.read();
        self.perm_repo
            .find_role_perm_ids(db, tenant_id, role_id)
            .await
    }

    pub async fn list_all_perms(
        &self,
        actor: &ActorContext,
        perm_type: Option<&str>,
    ) -> AppResult<Vec<PermissionTreeNode>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        let all = self.perm_repo.find_all(db, tenant_id).await?;
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
        let db = self.db.read();
        self.perm_repo
            .find_by_id(db, tenant_id, id)
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
        let saved = self.perm_repo.insert(db, tenant_id, model).await?;
        Ok(PermissionVo::from(saved))
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        command: UpdatePermissionCommand,
    ) -> AppResult<PermissionVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let mut model = self
            .perm_repo
            .find_by_id(db, tenant_id, command.id)
            .await?
            .ok_or_else(|| AppError::NotFound("权限不存在".into()))?;
        if model.code != command.code
            && self
                .perm_repo
                .find_by_code(db, tenant_id, &command.code)
                .await?
                .is_some()
        {
            return Err(AppError::Conflict("权限码已存在".into()));
        }
        model.name = command.name;
        model.code = command.code;
        model.parent_id = command.parent_id;
        model.perm_type = command.perm_type.as_str().to_owned();
        model.icon = command.icon;
        model.sort = command.sort;
        model.status = command.status;
        model.fill_on_update(&FillContext::new())?;
        let saved = self.perm_repo.update(db, tenant_id, model).await?;
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
        self.perm_repo.delete(db, tenant_id, id).await?;
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
        let mut created = 0usize;
        let mut missing = Vec::new();

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
            self.perm_repo.insert(db, tenant_id, model).await?;
            created += 1;
        }

        Ok(PermissionSyncReport {
            scanned: scanned_total,
            existing: existing_codes.len(),
            created,
            missing,
        })
    }
}
