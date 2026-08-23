use std::sync::Arc;

use crate::{
    AutoFill, ControlDatabaseCluster, DeptRepository, FillContext, Repository, RoleRepository,
    TenantConfigTransferRepository, TenantRepository, UserRepository,
    entities::{dept, role, user},
};
use ryframe_kernel::{AppError, DataScopeContext};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait, sea_query::LockType,
};

use ryframe_application::ports::users::{
    ManageableUserState, NewUserRecord, USER_STATUS_PENDING_ACTIVATION, UpdateUserRecord,
    UserAssignmentRole, UserAssignmentState, UserWritePersistencePort, UserWriteRecord,
    UserWriteTransaction,
};

use super::super::transaction::DatabasePortTransaction;

pub fn port(
    database: ControlDatabaseCluster,
    authorization_cache: ryframe_application::AuthorizationCache,
) -> Arc<dyn UserWritePersistencePort> {
    Arc::new(DatabaseUserWritePersistence {
        database,
        authorization_cache,
    })
}

struct DatabaseUserWritePersistence {
    database: ControlDatabaseCluster,
    authorization_cache: ryframe_application::AuthorizationCache,
}

#[async_trait::async_trait]
impl UserWritePersistencePort for DatabaseUserWritePersistence {
    async fn username_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
    ) -> ryframe_kernel::AppResult<bool> {
        Ok(UserRepository
            .find_by_username(self.database.write(), tenant_id, username)
            .await?
            .is_some())
    }

    async fn assignment_state<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: Option<i64>,
        role_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<UserAssignmentState> {
        let department_exists = match dept_id {
            Some(dept_id) => DeptRepository
                .find_by_id(self.database.write(), tenant_id, dept_id)
                .await?
                .is_some(),
            None => true,
        };
        let roles = if role_ids.is_empty() {
            Vec::new()
        } else {
            RoleRepository
                .find_by_ids(self.database.write(), tenant_id, role_ids)
                .await?
                .into_iter()
                .map(to_assignment_role)
                .collect()
        };
        Ok(UserAssignmentState {
            department_exists,
            roles,
        })
    }

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn UserWriteTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseUserWriteTransaction {
            transaction: transaction.into(),
            authorization_cache: self.authorization_cache.clone(),
        }) as Box<dyn UserWriteTransaction>)
    }
}

struct DatabaseUserWriteTransaction {
    transaction: DatabasePortTransaction,
    authorization_cache: ryframe_application::AuthorizationCache,
}

#[async_trait::async_trait]
impl UserWriteTransaction for DatabaseUserWriteTransaction {
    async fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()> {
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
            .await
            .map(|_| ())
    }

    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()> {
        TenantRepository
            .lock_tenant_in_txn(&self.transaction, tenant_id)
            .await
            .map(|_| ())
    }

    async fn assignment_state<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: Option<i64>,
        role_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<UserAssignmentState> {
        let department_exists = match dept_id {
            Some(dept_id) => dept::Entity::find_by_id(dept_id)
                .filter(dept::Column::TenantId.eq(tenant_id))
                .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .is_some(),
            None => true,
        };
        let roles = if role_ids.is_empty() {
            Vec::new()
        } else {
            role::Entity::find()
                .filter(role::Column::TenantId.eq(tenant_id))
                .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
                .filter(role::Column::Id.is_in(role_ids.iter().copied()))
                .order_by_asc(role::Column::Id)
                .lock(LockType::Update)
                .all(&self.transaction)
                .await
                .map_err(database_error)?
                .into_iter()
                .map(to_assignment_role)
                .collect()
        };
        Ok(UserAssignmentState {
            department_exists,
            roles,
        })
    }

    async fn ensure_user_quota<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()> {
        TenantRepository
            .ensure_user_quota_in_txn(&self.transaction, tenant_id)
            .await
    }

    async fn lock_manageable_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> ryframe_kernel::AppResult<Option<ManageableUserState>> {
        let Some(user) = UserRepository
            .find_by_id_with_data_scope_for_update(&self.transaction, tenant_id, user_id, scope)
            .await?
        else {
            return Ok(None);
        };
        let has_super_role = RoleRepository
            .user_has_super_role_in_txn(&self.transaction, tenant_id, user_id)
            .await?;
        Ok(Some(ManageableUserState {
            user: to_user_record(user),
            has_super_role,
        }))
    }

    async fn insert_user(&self, user: NewUserRecord) -> ryframe_kernel::AppResult<UserWriteRecord> {
        let mut model = user::Model {
            id: user.id,
            tenant_id: user.tenant_id,
            username: user.username,
            password_hash: user.password_hash,
            nickname: user.nickname,
            email: user.email,
            phone: user.phone,
            avatar: None,
            avatar_file_id: None,
            preferred_locale: None,
            status: USER_STATUS_PENDING_ACTIVATION.to_owned(),
            authorization_version: 1,
            dept_id: user.dept_id,
            remark: None,
            login_ip: None,
            login_date: None,
            del_flag: user::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        model.fill_on_insert(&FillContext::new())?;
        let saved = user::ActiveModel::from(model)
            .insert(&self.transaction)
            .await
            .map_err(database_error)?;
        Ok(to_user_record(saved))
    }

    async fn update_user<'a>(
        &'a self,
        tenant_id: &'a str,
        command: UpdateUserRecord,
    ) -> ryframe_kernel::AppResult<UserWriteRecord> {
        let mut model = UserRepository
            .find_by_id_for_update(&self.transaction, tenant_id, command.id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        model.nickname = command.nickname;
        model.email = command.email;
        model.phone = command.phone;
        model.dept_id = command.dept_id;
        model.fill_on_update(&FillContext::new())?;
        let saved = user::ActiveModel::from(model)
            .reset_all()
            .update(&self.transaction)
            .await
            .map_err(database_error)?;
        Ok(to_user_record(saved))
    }

    async fn update_status<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        status: String,
    ) -> ryframe_kernel::AppResult<()> {
        UserRepository
            .update_status(&self.transaction, tenant_id, user_id, status)
            .await
    }

    async fn replace_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        role_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<()> {
        RoleRepository
            .replace_roles_in_txn(&self.transaction, tenant_id, user_id, role_ids)
            .await
    }

    async fn increment_authorization_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<(i64, i32)>> {
        self.authorization_cache
            .increment_user_versions_in_transaction(&self.transaction, tenant_id, user_ids)
            .await
    }

    async fn delete_users<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<u64> {
        UserRepository
            .delete_many(&self.transaction, tenant_id, user_ids)
            .await
    }
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for DatabaseUserWriteTransaction {
    async fn commit(
        self: Box<Self>,
        audit_mode: ryframe_application::TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        match audit_mode {
            ryframe_application::TransactionAuditMode::CurrentRequest => {
                self.transaction.commit_audited().await
            }
            ryframe_application::TransactionAuditMode::Skip => {
                self.transaction.commit().await.map_err(database_error)
            }
        }
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
    }
}

fn to_assignment_role(role: role::Model) -> UserAssignmentRole {
    UserAssignmentRole {
        status_normal: role.status == role::Model::STATUS_NORMAL,
        is_super: role.is_super == 1,
    }
}

pub fn to_user_record(user: user::Model) -> UserWriteRecord {
    UserWriteRecord {
        id: user.id,
        username: user.username,
        nickname: user.nickname,
        email: user.email,
        phone: user.phone,
        avatar: user.avatar,
        status: user.status,
        dept_id: user.dept_id,
        remark: user.remark,
        created_at: user.created_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
