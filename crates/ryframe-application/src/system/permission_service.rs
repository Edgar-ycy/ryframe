use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use ryframe_kernel::{ActorContext, AppError, AppResult};

use crate::{
    AuthorizationCache, PermissionReadPort, PermissionRecord, PermissionWritePort,
    PermissionWriteTransaction,
};

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
    read: Arc<dyn PermissionReadPort>,
    write: Arc<dyn PermissionWritePort>,
    authorization_cache: AuthorizationCache,
}

impl PermissionService {
    pub fn new(
        read: Arc<dyn PermissionReadPort>,
        write: Arc<dyn PermissionWritePort>,
        authorization_cache: AuthorizationCache,
    ) -> Self {
        Self {
            read,
            write,
            authorization_cache,
        }
    }

    pub async fn find_role_permission_codes(
        &self,
        actor: &ActorContext,
        role_ids: &[i64],
    ) -> AppResult<Vec<String>> {
        self.read
            .find_role_codes(crate::validated_tenant_id(actor)?, role_ids)
            .await
    }

    pub async fn find_role_permission_ids(
        &self,
        actor: &ActorContext,
        role_id: i64,
    ) -> AppResult<Vec<i64>> {
        self.read
            .find_role_ids(crate::validated_tenant_id(actor)?, role_id)
            .await
    }

    pub async fn list_all_perms(
        &self,
        actor: &ActorContext,
        perm_type: Option<&str>,
    ) -> AppResult<Vec<PermissionTreeNode>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let permissions = self
            .read
            .find_all(tenant_id)
            .await?
            .into_iter()
            .filter(|permission| perm_type.is_none_or(|kind| permission.perm_type == kind))
            .collect();
        Ok(build_perm_tree(permissions))
    }

    pub async fn find_by_id(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<Option<PermissionVo>> {
        self.read
            .find_by_id(crate::validated_tenant_id(actor)?, id)
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
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        if transaction
            .find_by_code_for_update(tenant_id, &command.code)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("权限码已存在".into()));
        }
        if let Some(parent_id) = command.parent_id
            && transaction
                .find_by_id_for_update(tenant_id, parent_id)
                .await?
                .is_none()
        {
            return Err(AppError::Validation("父权限不存在".into()));
        }
        let saved = transaction
            .insert(
                tenant_id,
                PermissionRecord {
                    id: crate::next_id()?,
                    name: command.name,
                    code: command.code,
                    parent_id: command.parent_id,
                    perm_type: command.perm_type.as_str().into(),
                    icon: command.icon,
                    sort: command.sort,
                    status: command.status,
                    created_at: Default::default(),
                    updated_at: Default::default(),
                },
            )
            .await?;
        self.commit_mutation(transaction, tenant_id).await?;
        Ok(saved.into())
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        command: UpdatePermissionCommand,
    ) -> AppResult<PermissionVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        ensure_tenant_permission_code_boundary(tenant_id, &command.code)?;
        if command.parent_id == Some(command.id) {
            return Err(AppError::Validation("权限不能将自己设为上级".into()));
        }
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let mut record = transaction
            .find_by_id_for_update(tenant_id, command.id)
            .await?
            .ok_or_else(|| AppError::NotFound("权限不存在".into()))?;
        if record.code != command.code
            && transaction
                .find_by_code_for_update(tenant_id, &command.code)
                .await?
                .is_some()
        {
            return Err(AppError::Conflict("权限码已存在".into()));
        }
        self.validate_parent_chain(
            transaction.as_ref(),
            tenant_id,
            command.id,
            command.parent_id,
        )
        .await?;
        record.name = command.name;
        record.code = command.code;
        record.parent_id = command.parent_id;
        record.perm_type = command.perm_type.as_str().into();
        record.icon = command.icon;
        record.sort = command.sort;
        record.status = command.status;
        let saved = transaction.update(tenant_id, record).await?;
        self.commit_mutation(transaction, tenant_id).await?;
        Ok(saved.into())
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        transaction
            .find_by_id_for_update(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("权限不存在".into()))?;
        if transaction.is_referenced(tenant_id, id).await? {
            return Err(AppError::Conflict(
                "权限仍被角色或菜单引用，不能删除".into(),
            ));
        }
        transaction.delete(tenant_id, id).await?;
        self.commit_mutation(transaction, tenant_id).await
    }

    pub async fn sync_route_permissions(
        &self,
        actor: &ActorContext,
        route_permission_codes: &[&str],
    ) -> AppResult<PermissionSyncReport> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scanned = route_permission_codes
            .iter()
            .copied()
            .filter(|code| {
                tenant_id == SYSTEM_TENANT_ID
                    || (!is_tenant_permission_code(code) && !is_platform_permission_code(code))
            })
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let transaction = self.write.begin().await?;
        transaction.lock_configuration(tenant_id).await?;
        let scanned = transaction
            .filter_syncable_codes(tenant_id, scanned)
            .await?;
        let scanned_total = scanned.len();
        let existing_codes = transaction
            .find_all_for_update(tenant_id)
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect::<HashSet<_>>();
        let mut missing = Vec::new();
        for code in scanned {
            if existing_codes.contains(&code) {
                continue;
            }
            let name = code.rsplit(':').next().unwrap_or(&code).to_owned();
            let saved = transaction
                .insert(
                    tenant_id,
                    PermissionRecord {
                        id: crate::next_id()?,
                        name,
                        code,
                        parent_id: None,
                        perm_type: PermissionType::Api.as_str().into(),
                        icon: None,
                        sort: 0,
                        status: "1".into(),
                        created_at: Default::default(),
                        updated_at: Default::default(),
                    },
                )
                .await?;
            missing.push(saved.code);
        }
        let created = missing.len();
        if created == 0 {
            transaction.rollback().await?;
        } else {
            self.commit_mutation(transaction, tenant_id).await?;
        }
        Ok(PermissionSyncReport {
            scanned: scanned_total,
            existing: existing_codes.len(),
            created,
            missing,
        })
    }

    async fn validate_parent_chain(
        &self,
        transaction: &dyn PermissionWriteTransaction,
        tenant_id: &str,
        id: i64,
        mut parent_id: Option<i64>,
    ) -> AppResult<()> {
        let mut visited = HashSet::new();
        while let Some(current_parent_id) = parent_id {
            if !visited.insert(current_parent_id) {
                return Err(AppError::Internal("权限父级链存在循环".into()));
            }
            let parent = transaction
                .find_by_id_for_update(tenant_id, current_parent_id)
                .await?
                .ok_or_else(|| AppError::Validation("父权限不存在".into()))?;
            if parent.id == id {
                return Err(AppError::Validation(
                    "不能将权限移动到自己的后代节点".into(),
                ));
            }
            parent_id = parent.parent_id;
        }
        Ok(())
    }

    async fn commit_mutation(
        &self,
        transaction: Box<dyn PermissionWriteTransaction>,
        tenant_id: &str,
    ) -> AppResult<()> {
        let authorization_epoch = transaction.increment_authorization_epoch(tenant_id).await?;
        transaction
            .increment_configuration_version(tenant_id)
            .await?;
        transaction.commit().await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, authorization_epoch)
            .await
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

#[cfg(test)]
mod tests {
    use super::{ensure_tenant_permission_code_boundary, is_platform_permission_code};

    #[test]
    fn tenant_permission_boundary_is_case_insensitive_and_fail_closed() {
        assert!(ensure_tenant_permission_code_boundary("system", "tenant:read").is_ok());
        assert!(ensure_tenant_permission_code_boundary("demo", "TENANT:read").is_err());
        assert!(is_platform_permission_code("PLATFORM:ops"));
    }
}
