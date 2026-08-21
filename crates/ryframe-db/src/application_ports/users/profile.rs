use std::sync::Arc;

use crate::{
    AutoFill, ControlDatabaseCluster, DeptRepository, FileRepository, FillContext,
    PermissionRepository, ReadConsistency, Repository, RoleRepository, TenantRepository,
    UserRepository,
    entities::{sys_file, user},
};
use sea_orm::{ActiveModelTrait, TransactionTrait};

use ryframe_application::{
    AuthorizationCache, ControlTransaction, PersistenceFuture,
    ports::users::{
        ProfileAvatarFile, ProfileAvatarState, ProfilePersistencePort, ProfileRecord,
        ProfileTransaction, ProfileUserState,
    },
};

use super::super::transaction::DatabasePortTransaction;

pub fn port(
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
) -> Arc<dyn ProfilePersistencePort> {
    Arc::new(DatabaseProfilePersistence {
        database,
        authorization_cache,
    })
}

struct DatabaseProfilePersistence {
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
}

struct DatabaseProfileTransaction {
    transaction: DatabasePortTransaction,
    authorization_cache: AuthorizationCache,
}

impl ProfilePersistencePort for DatabaseProfilePersistence {
    fn find_profile<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<ProfileRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let Some(user) = UserRepository
                .find_by_id(&database, tenant_id, user_id)
                .await?
            else {
                return Ok(None);
            };
            let dept_name = if let Some(dept_id) = user.dept_id {
                DeptRepository
                    .find_by_id(&database, tenant_id, dept_id)
                    .await?
                    .map(|department| department.name)
            } else {
                None
            };
            let roles = RoleRepository
                .find_user_roles(&database, tenant_id, user.id)
                .await?;
            let (role_ids, role_codes): (Vec<_>, Vec<_>) =
                roles.into_iter().map(|role| (role.id, role.code)).unzip();
            let permissions = PermissionRepository
                .find_role_perms(&database, tenant_id, &role_ids)
                .await?
                .into_iter()
                .map(|permission| permission.code)
                .collect();
            Ok(Some(ProfileRecord {
                user_id: user.id,
                username: user.username,
                nickname: user.nickname,
                email: user.email,
                phone: user.phone,
                avatar: user.avatar,
                preferred_locale: user.preferred_locale,
                dept_id: user.dept_id,
                dept_name,
                status: user.status,
                remark: user.remark,
                login_ip: user.login_ip,
                login_date: user.login_date,
                created_at: user.created_at,
                roles: role_codes,
                permissions,
            }))
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ProfileTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseProfileTransaction {
                transaction: transaction.into(),
                authorization_cache: self.authorization_cache.clone(),
            }) as Box<dyn ProfileTransaction>)
        })
    }
}

impl ProfileTransaction for DatabaseProfileTransaction {
    fn find_user_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<ProfileUserState>> {
        Box::pin(async move {
            Ok(UserRepository
                .find_by_id_for_update(&self.transaction, tenant_id, user_id)
                .await?
                .map(|user| ProfileUserState {
                    password_hash: user.password_hash,
                    avatar_file_id: user.avatar_file_id,
                }))
        })
    }

    fn update_profile<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        nickname: String,
        email: String,
        phone: String,
        preferred_locale: Option<String>,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            let mut user = UserRepository
                .find_by_id_for_update(&self.transaction, tenant_id, user_id)
                .await?
                .ok_or_else(|| ryframe_kernel::AppError::NotFound("用户不存在".into()))?;
            user.nickname = nickname;
            user.email = email;
            user.phone = phone;
            user.preferred_locale = preferred_locale;
            user.fill_on_update(&FillContext::new())?;
            user::ActiveModel::from(user)
                .reset_all()
                .update(&self.transaction)
                .await
                .map(|_| ())
                .map_err(database_error)
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

    fn update_password<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        password_hash: String,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            let mut user = UserRepository
                .find_by_id_for_update(&self.transaction, tenant_id, user_id)
                .await?
                .ok_or_else(|| ryframe_kernel::AppError::NotFound("用户不存在".into()))?;
            user.password_hash = password_hash;
            user.fill_on_update(&FillContext::new())?;
            user::ActiveModel::from(user)
                .reset_all()
                .update(&self.transaction)
                .await
                .map(|_| ())
                .map_err(database_error)
        })
    }

    fn increment_user_authorization_version<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Vec<(i64, i32)>> {
        Box::pin(async move {
            self.authorization_cache
                .increment_user_versions_in_transaction(&self.transaction, tenant_id, &[user_id])
                .await
        })
    }

    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move { FileRepository.database_utc_now(&self.transaction).await })
    }

    fn find_avatar_file_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, Option<ProfileAvatarFile>> {
        Box::pin(async move {
            Ok(FileRepository
                .find_by_id_any_status_for_update(&self.transaction, tenant_id, file_id)
                .await?
                .map(|file| ProfileAvatarFile {
                    bucket: file.bucket,
                    state: if file.upload_status == sys_file::Model::UPLOAD_STATUS_CLEANUP {
                        ProfileAvatarState::Cleanup
                    } else if file.upload_status == sys_file::Model::UPLOAD_STATUS_READY
                        && file.del_flag == sys_file::Model::DEL_FLAG_NORMAL
                    {
                        ProfileAvatarState::Ready
                    } else {
                        ProfileAvatarState::Unavailable
                    },
                }))
        })
    }

    fn restore_avatar_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            FileRepository
                .restore_avatar_file_for_reference_in_txn(
                    &self.transaction,
                    tenant_id,
                    file_id,
                    now,
                )
                .await
        })
    }

    fn update_avatar<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        avatar_url: String,
        avatar_file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            UserRepository
                .update_avatar_in_txn(
                    &self.transaction,
                    tenant_id,
                    user_id,
                    avatar_url,
                    avatar_file_id,
                    now,
                )
                .await
        })
    }

    fn count_avatar_references<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            UserRepository
                .count_avatar_file_references_in_txn(&self.transaction, tenant_id, file_id)
                .await
        })
    }

    fn mark_avatar_orphan<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
        cleanup_after: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            FileRepository
                .mark_avatar_orphan_for_cleanup_in_txn(
                    &self.transaction,
                    tenant_id,
                    file_id,
                    now,
                    cleanup_after,
                )
                .await
        })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

impl ControlTransaction for DatabaseProfileTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.commit_audited().await })
    }
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}
