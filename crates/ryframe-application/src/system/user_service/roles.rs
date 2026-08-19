use ryframe_db::{Repository, TenantConfigTransferRepository};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
    sea_query::LockType,
};

use super::UserService;

impl UserService {
    pub async fn replace_roles(
        &self,
        actor: &ActorContext,
        user_id: i64,
        mut role_ids: Vec<i64>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
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
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        self.validate_assignments_in_txn(&transaction, actor, None, Some(&role_ids))
            .await?;
        self.lock_manageable_user_in_txn(actor, &transaction, user_id)
            .await?;
        self.role_repo
            .replace_roles_in_txn(&transaction, tenant_id, user_id, &role_ids)
            .await?;
        let versions = self
            .invalidate_sessions_for_tenant_in_txn(&transaction, tenant_id, &[user_id])
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_user_versions(tenant_id, &versions)
            .await?;
        Ok(())
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
            if roles
                .iter()
                .any(|role| role.status != ryframe_db::entities::role::Model::STATUS_NORMAL)
            {
                return Err(AppError::Validation("不能分配已停用的角色".into()));
            }
            if !actor.is_super_admin && roles.iter().any(|role| role.is_super == 1) {
                return Err(AppError::Authorization("无权限分配超级管理员角色".into()));
            }
        }
        Ok(())
    }

    pub(super) async fn validate_assignments_in_txn(
        &self,
        transaction: &sea_orm::DatabaseTransaction,
        actor: &ActorContext,
        dept_id: Option<i64>,
        role_ids: Option<&[i64]>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if let Some(dept_id) = dept_id
            && ryframe_db::entities::dept::Entity::find_by_id(dept_id)
                .filter(ryframe_db::entities::dept::Column::TenantId.eq(tenant_id))
                .filter(
                    ryframe_db::entities::dept::Column::DelFlag
                        .eq(ryframe_db::entities::dept::Model::DEL_FLAG_NORMAL),
                )
                .lock(LockType::Update)
                .one(transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?
                .is_none()
        {
            return Err(AppError::Validation("部门不存在或不属于当前租户".into()));
        }
        if let Some(role_ids) = role_ids {
            let mut role_ids = role_ids.to_vec();
            role_ids.sort_unstable();
            role_ids.dedup();
            let roles = ryframe_db::entities::role::Entity::find()
                .filter(ryframe_db::entities::role::Column::TenantId.eq(tenant_id))
                .filter(
                    ryframe_db::entities::role::Column::DelFlag
                        .eq(ryframe_db::entities::role::Model::DEL_FLAG_NORMAL),
                )
                .filter(ryframe_db::entities::role::Column::Id.is_in(role_ids.iter().copied()))
                .order_by_asc(ryframe_db::entities::role::Column::Id)
                .lock(LockType::Update)
                .all(transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
            if roles.len() != role_ids.len() {
                return Err(AppError::Validation("角色不存在或不属于当前租户".into()));
            }
            if roles
                .iter()
                .any(|role| role.status != ryframe_db::entities::role::Model::STATUS_NORMAL)
            {
                return Err(AppError::Validation("不能分配已停用的角色".into()));
            }
            if !actor.is_super_admin && roles.iter().any(|role| role.is_super == 1) {
                return Err(AppError::Authorization("无权限分配超级管理员角色".into()));
            }
        }
        Ok(())
    }
}
