use ryframe_common::{ActorContext, AppError, AppResult};
use ryframe_core::Repository;
use sea_orm::TransactionTrait;

use super::UserService;

impl UserService {
    pub async fn replace_roles(
        &self,
        actor: &ActorContext,
        user_id: i64,
        mut role_ids: Vec<i64>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_user_accessible(actor, user_id).await?;
        self.ensure_not_super_admin_user(actor, user_id).await?;
        self.validate_assignments(actor, None, Some(&role_ids))
            .await?;

        role_ids.sort_unstable();
        role_ids.dedup();
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        self.role_repo
            .replace_roles_in_txn(&transaction, tenant_id, user_id, &role_ids)
            .await?;
        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        self.invalidate_permission_cache(tenant_id, user_id).await;
        self.invalidate_sessions_for_tenant(tenant_id, &[user_id])
            .await
    }

    pub(super) async fn validate_assignments(
        &self,
        actor: &ActorContext,
        dept_id: Option<i64>,
        role_ids: Option<&[i64]>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if let Some(dept_id) = dept_id
            && self
                .dept_repo
                .find_by_id(self.db.write(), tenant_id, dept_id)
                .await?
                .is_none()
        {
            return Err(AppError::Validation("部门不存在或不属于当前租户".into()));
        }
        if let Some(role_ids) = role_ids {
            let mut role_ids = role_ids.to_vec();
            role_ids.sort_unstable();
            role_ids.dedup();
            let roles = self
                .role_repo
                .find_by_ids(self.db.write(), tenant_id, &role_ids)
                .await?;
            if roles.len() != role_ids.len() {
                return Err(AppError::Validation("角色不存在或不属于当前租户".into()));
            }
            if !actor.is_super_admin && roles.iter().any(|role| role.is_super == 1) {
                return Err(AppError::Authorization("无权限分配超级管理员角色".into()));
            }
        }
        Ok(())
    }
}
