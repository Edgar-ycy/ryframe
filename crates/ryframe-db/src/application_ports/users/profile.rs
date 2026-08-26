use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    AutoFill, ControlDatabaseCluster, DeptRepository, FileRepository, FillContext,
    PermissionRepository, ReadConsistency, Repository, RoleRepository, TenantRepository,
    UserRepository,
    entities::{sys_file, user},
};
use sea_orm::{ActiveModelTrait, TransactionTrait};

use ryframe_application::{
    AuthorizationCache, PersistenceTransaction, TransactionAuditMode,
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

#[async_trait::async_trait]
impl ProfilePersistencePort for DatabaseProfilePersistence {
    async fn find_profile<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ProfileRecord>> {
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
    }

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ProfileTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseProfileTransaction {
            transaction: transaction.into(),
            authorization_cache: self.authorization_cache.clone(),
        }) as Box<dyn ProfileTransaction>)
    }
}

#[async_trait::async_trait]
impl ProfileTransaction for DatabaseProfileTransaction {
    async fn find_user_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ProfileUserState>> {
        Ok(UserRepository
            .find_by_id_for_update(&self.transaction, tenant_id, user_id)
            .await?
            .map(|user| ProfileUserState {
                password_hash: user.password_hash,
                avatar_file_id: user.avatar_file_id,
            }))
    }

    async fn update_profile<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        nickname: String,
        email: String,
        phone: String,
        preferred_locale: Option<String>,
    ) -> ryframe_kernel::AppResult<()> {
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
            .db()
    }

    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()> {
        TenantRepository
            .lock_tenant_in_txn(&self.transaction, tenant_id)
            .await
            .map(|_| ())
    }

    async fn update_password<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        password_hash: String,
    ) -> ryframe_kernel::AppResult<()> {
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
            .db()
    }

    async fn increment_user_authorization_version<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<(i64, i32)>> {
        self.authorization_cache
            .increment_user_versions_in_transaction(&self.transaction, tenant_id, &[user_id])
            .await
    }

    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn find_avatar_file_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ProfileAvatarFile>> {
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
    }

    async fn restore_avatar_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .restore_avatar_file_for_reference_in_txn(&self.transaction, tenant_id, file_id, now)
            .await
    }

    async fn update_avatar<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        avatar_url: String,
        avatar_file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<()> {
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
    }

    async fn count_avatar_references<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<u64> {
        UserRepository
            .count_avatar_file_references_in_txn(&self.transaction, tenant_id, file_id)
            .await
    }

    async fn mark_avatar_orphan<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
        cleanup_after: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .mark_avatar_orphan_for_cleanup_in_txn(
                &self.transaction,
                tenant_id,
                file_id,
                now,
                cleanup_after,
            )
            .await
    }
}

#[async_trait::async_trait]
impl PersistenceTransaction for DatabaseProfileTransaction {
    async fn commit(
        self: Box<Self>,
        audit_mode: TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        match audit_mode {
            TransactionAuditMode::CurrentRequest => self.transaction.commit_audited().await,
            TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.db()
    }
}
