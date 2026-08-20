use std::sync::Arc;

use ryframe_db::{
    AutoFill, ControlDatabaseCluster, FillContext, PasswordResetRequestRepository, Repository,
    RoleRepository, TenantRepository, UserRepository,
    entities::{password_reset_request, role, user, user_role},
};
use ryframe_kernel::{AppError, DataScopeContext};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseTransaction, EntityTrait, ExprTrait, QueryFilter,
    QueryOrder, QuerySelect, TransactionTrait,
    sea_query::{Expr, LockType},
};

use crate::{
    NewPasswordResetRequest, PASSWORD_RESET_STATUS_PENDING, PasswordResetPersistencePort,
    PasswordResetRequestRecord, PasswordResetTransaction, PasswordResetUserState,
    PersistenceFuture,
};

pub fn port(
    database: ControlDatabaseCluster,
    authorization_cache: crate::AuthorizationCache,
) -> Arc<dyn PasswordResetPersistencePort> {
    Arc::new(LegacyPasswordResetPersistence {
        database,
        authorization_cache,
    })
}

struct LegacyPasswordResetPersistence {
    database: ControlDatabaseCluster,
    authorization_cache: crate::AuthorizationCache,
}

impl PasswordResetPersistencePort for LegacyPasswordResetPersistence {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            PasswordResetRequestRepository
                .database_utc_now(self.database.write())
                .await
        })
    }

    fn find_request<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
    ) -> PersistenceFuture<'a, Option<PasswordResetRequestRecord>> {
        Box::pin(async move {
            Ok(PasswordResetRequestRepository
                .find_by_id(self.database.write(), tenant_id, request_id)
                .await?
                .map(to_request_record))
        })
    }

    fn find_user_state<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<PasswordResetUserState>> {
        Box::pin(async move {
            let Some(user) = UserRepository
                .find_by_id(self.database.write(), tenant_id, user_id)
                .await?
            else {
                return Ok(None);
            };
            let has_super_role = RoleRepository
                .find_user_roles_all_status(self.database.write(), tenant_id, user_id)
                .await?
                .iter()
                .any(|role| role.is_super == 1);
            Ok(Some(to_user_state(user, has_super_role)))
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn PasswordResetTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(LegacyPasswordResetTransaction {
                transaction,
                authorization_cache: self.authorization_cache.clone(),
            }) as Box<dyn PasswordResetTransaction>)
        })
    }
}

struct LegacyPasswordResetTransaction {
    transaction: DatabaseTransaction,
    authorization_cache: crate::AuthorizationCache,
}

impl PasswordResetTransaction for LegacyPasswordResetTransaction {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            PasswordResetRequestRepository
                .database_utc_now(&self.transaction)
                .await
        })
    }

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantRepository
                .lock_tenant_in_txn(&self.transaction, tenant_id)
                .await
                .map(|_| ())
        })
    }

    fn lock_manageable_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, Option<PasswordResetUserState>> {
        Box::pin(async move {
            let Some(user) = UserRepository
                .find_by_id_with_data_scope_for_update(&self.transaction, tenant_id, user_id, scope)
                .await?
            else {
                return Ok(None);
            };
            let has_super_role = RoleRepository
                .user_has_super_role_in_txn(&self.transaction, tenant_id, user_id)
                .await?;
            Ok(Some(to_user_state(user, has_super_role)))
        })
    }

    fn insert_request(
        &self,
        request: NewPasswordResetRequest,
    ) -> PersistenceFuture<'_, PasswordResetRequestRecord> {
        Box::pin(async move {
            let mut model = password_reset_request::Model {
                id: request.id,
                tenant_id: request.tenant_id,
                target_user_id: request.target_user_id,
                requested_by: request.requested_by,
                reason: request.reason,
                token_hash: request.token_hash,
                expires_at: request.expires_at,
                completed_at: None,
                request_ip: request.request_ip,
                status: PASSWORD_RESET_STATUS_PENDING.to_owned(),
                created_at: Default::default(),
                updated_at: Default::default(),
            };
            model.fill_on_insert(&FillContext::new())?;
            let saved = password_reset_request::ActiveModel::from(model)
                .insert(&self.transaction)
                .await
                .map_err(database_error)?;
            Ok(to_request_record(saved))
        })
    }

    fn lock_request<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
    ) -> PersistenceFuture<'a, Option<PasswordResetRequestRecord>> {
        Box::pin(async move {
            Ok(password_reset_request::Entity::find_by_id(request_id)
                .filter(password_reset_request::Column::TenantId.eq(tenant_id))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .map(to_request_record))
        })
    }

    fn lock_user_state<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<PasswordResetUserState>> {
        Box::pin(async move {
            let Some(user) = user::Entity::find_by_id(user_id)
                .filter(user::Column::TenantId.eq(tenant_id))
                .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
            else {
                return Ok(None);
            };
            let role_ids = user_role::Entity::find()
                .filter(user_role::Column::TenantId.eq(tenant_id))
                .filter(user_role::Column::UserId.eq(user_id))
                .order_by_asc(user_role::Column::RoleId)
                .lock(LockType::Update)
                .all(&self.transaction)
                .await
                .map_err(database_error)?
                .into_iter()
                .map(|relation| relation.role_id)
                .collect::<Vec<_>>();
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
            };
            if roles.len() != role_ids.len() {
                return Err(AppError::Conflict(
                    "用户角色状态已发生变化，请重新发起密码重置".into(),
                ));
            }
            let has_super_role = roles.iter().any(|role| role.is_super == 1);
            Ok(Some(to_user_state(user, has_super_role)))
        })
    }

    fn expire_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
        evaluated_at: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            let result = password_reset_request::Entity::update_many()
                .col_expr(
                    password_reset_request::Column::Status,
                    Expr::value(password_reset_request::Model::STATUS_EXPIRED),
                )
                .col_expr(
                    password_reset_request::Column::UpdatedAt,
                    Expr::value(evaluated_at),
                )
                .filter(password_reset_request::Column::Id.eq(request_id))
                .filter(password_reset_request::Column::TenantId.eq(tenant_id))
                .filter(password_reset_request::Column::Status.eq(PASSWORD_RESET_STATUS_PENDING))
                .filter(password_reset_request::Column::CompletedAt.is_null())
                .filter(password_reset_request::Column::ExpiresAt.lte(evaluated_at))
                .exec(&self.transaction)
                .await
                .map_err(database_error)?;
            Ok(result.rows_affected == 1)
        })
    }

    fn complete_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
        completed_at: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            PasswordResetRequestRepository
                .complete_pending_in_txn(&self.transaction, tenant_id, request_id, completed_at)
                .await
        })
    }

    fn update_password<'a>(
        &'a self,
        tenant_id: &'a str,
        expected: &'a PasswordResetUserState,
        password_hash: String,
        next_status: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            let result = user::Entity::update_many()
                .col_expr(user::Column::PasswordHash, Expr::value(password_hash))
                .col_expr(
                    user::Column::AuthorizationVersion,
                    Expr::col(user::Column::AuthorizationVersion).add(1),
                )
                .col_expr(user::Column::Status, Expr::value(next_status))
                .col_expr(user::Column::UpdatedAt, Expr::value(updated_at))
                .filter(user::Column::Id.eq(expected.id))
                .filter(user::Column::TenantId.eq(tenant_id))
                .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
                .filter(user::Column::Status.eq(expected.status.as_str()))
                .filter(user::Column::AuthorizationVersion.eq(expected.authorization_version))
                .exec(&self.transaction)
                .await
                .map_err(database_error)?;
            Ok(result.rows_affected == 1)
        })
    }

    fn record_user_mirror_update<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        authorization_version: i32,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            self.authorization_cache
                .record_user_mirror_update_in_transaction(
                    &self.transaction,
                    tenant_id,
                    user_id,
                    authorization_version,
                )
                .await
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { crate::commit_current_audit(self.transaction).await })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn to_request_record(model: password_reset_request::Model) -> PasswordResetRequestRecord {
    PasswordResetRequestRecord {
        id: model.id,
        tenant_id: model.tenant_id,
        target_user_id: model.target_user_id,
        token_hash: model.token_hash,
        expires_at: model.expires_at,
        completed_at: model.completed_at,
        status: model.status,
    }
}

fn to_user_state(model: user::Model, has_super_role: bool) -> PasswordResetUserState {
    PasswordResetUserState {
        id: model.id,
        authorization_version: model.authorization_version,
        status: model.status,
        has_super_role,
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
