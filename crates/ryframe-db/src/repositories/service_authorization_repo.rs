use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    sea_query::LockType,
};

use crate::entities::{
    dept, permission, role, role_dept, role_permission, service_account_role, user, user_role,
};

/// 在同一事务、固定锁顺序下取得的双主体授权关系快照。
#[derive(Clone, Debug)]
pub struct ServiceAuthorizationSnapshot {
    pub user: Option<user::Model>,
    pub account_role_ids: Vec<i64>,
    pub user_role_ids: Vec<i64>,
    pub roles: Vec<role::Model>,
    pub role_permissions: Vec<role_permission::Model>,
    pub permissions: Vec<permission::Model>,
    pub role_departments: Vec<role_dept::Model>,
    pub departments: Vec<dept::Model>,
}

pub struct ServiceAuthorizationRepository;

impl ServiceAuthorizationRepository {
    /// 调用前必须依次持有租户、账号、凭据、委托行的共享锁。
    ///
    /// 本方法继续按用户、账号/用户角色关系、角色、角色权限、权限、角色部门、部门的
    /// 顺序加共享锁。所有 Agent 查询在提交审计前必须保持该事务不结束。
    pub async fn lock_snapshot_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
        represented_user_id: Option<i64>,
    ) -> AppResult<ServiceAuthorizationSnapshot> {
        let user = if let Some(user_id) = represented_user_id {
            Some(
                user::Entity::find_by_id(user_id)
                    .filter(user::Column::TenantId.eq(tenant_id))
                    .lock(LockType::Share)
                    .one(txn)
                    .await
                    .map_err(database_error)?
                    .ok_or_else(|| AppError::NotFound("委托用户不存在".into()))?,
            )
        } else {
            None
        };
        let account_relations = service_account_role::Entity::find()
            .filter(service_account_role::Column::TenantId.eq(tenant_id))
            .filter(service_account_role::Column::AccountId.eq(account_id))
            .order_by_asc(service_account_role::Column::RoleId)
            .lock(LockType::Share)
            .all(txn)
            .await
            .map_err(database_error)?;
        let user_relations = if let Some(user_id) = represented_user_id {
            user_role::Entity::find()
                .filter(user_role::Column::TenantId.eq(tenant_id))
                .filter(user_role::Column::UserId.eq(user_id))
                .order_by_asc(user_role::Column::RoleId)
                .lock(LockType::Share)
                .all(txn)
                .await
                .map_err(database_error)?
        } else {
            Vec::new()
        };
        let account_role_ids = account_relations
            .into_iter()
            .map(|relation| relation.role_id)
            .collect::<Vec<_>>();
        let user_role_ids = user_relations
            .into_iter()
            .map(|relation| relation.role_id)
            .collect::<Vec<_>>();
        let mut role_ids = account_role_ids
            .iter()
            .chain(&user_role_ids)
            .copied()
            .collect::<Vec<_>>();
        role_ids.sort_unstable();
        role_ids.dedup();
        let roles = if role_ids.is_empty() {
            Vec::new()
        } else {
            role::Entity::find()
                .filter(role::Column::TenantId.eq(tenant_id))
                .filter(role::Column::Id.is_in(role_ids.iter().copied()))
                .order_by_asc(role::Column::Id)
                .lock(LockType::Share)
                .all(txn)
                .await
                .map_err(database_error)?
        };
        let permission_relations = if role_ids.is_empty() {
            Vec::new()
        } else {
            role_permission::Entity::find()
                .filter(role_permission::Column::TenantId.eq(tenant_id))
                .filter(role_permission::Column::RoleId.is_in(role_ids.iter().copied()))
                .order_by_asc(role_permission::Column::RoleId)
                .order_by_asc(role_permission::Column::PermId)
                .lock(LockType::Share)
                .all(txn)
                .await
                .map_err(database_error)?
        };
        let mut permission_ids = permission_relations
            .iter()
            .map(|relation| relation.perm_id)
            .collect::<Vec<_>>();
        permission_ids.sort_unstable();
        permission_ids.dedup();
        let permissions = if permission_ids.is_empty() {
            Vec::new()
        } else {
            permission::Entity::find()
                .filter(permission::Column::TenantId.eq(tenant_id))
                .filter(permission::Column::Id.is_in(permission_ids))
                .order_by_asc(permission::Column::Id)
                .lock(LockType::Share)
                .all(txn)
                .await
                .map_err(database_error)?
        };
        let role_departments = if role_ids.is_empty() {
            Vec::new()
        } else {
            role_dept::Entity::find()
                .filter(role_dept::Column::TenantId.eq(tenant_id))
                .filter(role_dept::Column::RoleId.is_in(role_ids))
                .order_by_asc(role_dept::Column::RoleId)
                .order_by_asc(role_dept::Column::DeptId)
                .lock(LockType::Share)
                .all(txn)
                .await
                .map_err(database_error)?
        };
        let departments = dept::Entity::find()
            .filter(dept::Column::TenantId.eq(tenant_id))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .order_by_asc(dept::Column::Id)
            .lock(LockType::Share)
            .all(txn)
            .await
            .map_err(database_error)?;
        Ok(ServiceAuthorizationSnapshot {
            user,
            account_role_ids,
            user_role_ids,
            roles,
            role_permissions: permission_relations,
            permissions,
            role_departments,
            departments,
        })
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
