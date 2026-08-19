use ryframe_adapters::auto_fill::{AutoFill, FillContext};
use ryframe_auth::password;
use ryframe_db::{TenantConfigTransferRepository, TenantRepository, entities::user};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{ActiveModelTrait, DatabaseTransaction, TransactionTrait};
use uuid::Uuid;

use super::{CreateUserParams, UpdateUserParams, UserService, UserVo};

impl UserService {
    pub async fn create(
        &self,
        actor: &ActorContext,
        params: CreateUserParams<'_>,
    ) -> AppResult<UserVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let CreateUserParams {
            username,
            nickname,
            email,
            phone,
            dept_id,
            mut role_ids,
        } = params;
        normalize_ids(&mut role_ids);
        self.validate_assignments(actor, dept_id, Some(&role_ids))
            .await?;

        if self
            .user_repo
            .find_by_username(self.db.write(), tenant_id, username)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("用户名已存在".into()));
        }

        let activation_secret = format!("pending:{}", Uuid::new_v4());
        let mut user = user::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            username: username.to_owned(),
            password_hash: password::hash(&activation_secret)?,
            nickname: nickname.to_owned(),
            email: email.to_owned(),
            phone: phone.to_owned(),
            avatar: None,
            avatar_file_id: None,
            preferred_locale: None,
            status: user::Model::STATUS_PENDING_ACTIVATION.into(),
            authorization_version: 1,
            dept_id,
            remark: None,
            login_ip: None,
            login_date: None,
            del_flag: user::Model::DEL_FLAG_NORMAL.into(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        user.fill_on_insert(&FillContext::new())?;

        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        // 部门和角色在配置回滚期间必须保持存在；栅栏后重新校验，避免使用事务前快照。
        self.validate_assignments_in_txn(&transaction, actor, dept_id, Some(&role_ids))
            .await?;
        TenantRepository
            .ensure_user_quota_in_txn(&transaction, tenant_id)
            .await?;
        let active: user::ActiveModel = user.into();
        let saved = active
            .insert(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if !role_ids.is_empty() {
            self.role_repo
                .replace_roles_in_txn(&transaction, tenant_id, saved.id, &role_ids)
                .await?;
        }
        crate::commit_current_audit(transaction).await?;
        Ok(UserVo::from(saved))
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        params: UpdateUserParams<'_>,
    ) -> AppResult<UserVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let UpdateUserParams {
            id,
            nickname,
            email,
            phone,
            dept_id,
        } = params;
        self.validate_assignments(actor, dept_id, None).await?;
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        self.validate_assignments_in_txn(&transaction, actor, dept_id, None)
            .await?;
        let mut user = self
            .lock_manageable_user_in_txn(actor, &transaction, id)
            .await?;
        user.nickname = nickname.to_owned();
        user.email = email.to_owned();
        user.phone = phone.to_owned();
        user.dept_id = dept_id;
        user.fill_on_update(&FillContext::new())?;

        let active: user::ActiveModel = user.into();
        let saved = active
            .reset_all()
            .update(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let versions = self
            .invalidate_sessions_for_tenant_in_txn(&transaction, tenant_id, &[saved.id])
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_user_versions(tenant_id, &versions)
            .await?;
        Ok(UserVo::from(saved))
    }

    pub async fn update_status(
        &self,
        actor: &ActorContext,
        id: i64,
        status: String,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_manageable_status(&status)?;
        if id == actor.user_id && status != user::Model::STATUS_NORMAL {
            return Err(AppError::Authorization("禁止停用自己".into()));
        }
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, tenant_id)
            .await?;
        self.lock_manageable_user_in_txn(actor, &transaction, id)
            .await?;
        self.user_repo
            .update_status(&transaction, tenant_id, id, status)
            .await?;
        let versions = self
            .invalidate_sessions_for_tenant_in_txn(&transaction, tenant_id, &[id])
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_user_versions(tenant_id, &versions)
            .await
    }

    pub async fn delete_many(&self, actor: &ActorContext, ids: &[i64]) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if ids.is_empty() {
            return Err(AppError::Validation("请选择要删除的用户".into()));
        }
        let mut ids = ids.to_vec();
        normalize_ids(&mut ids);
        for id in &ids {
            if *id == actor.user_id {
                return Err(AppError::Authorization("禁止删除自己".into()));
            }
        }
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, tenant_id)
            .await?;
        // 始终按升序获取用户锁，避免重叠的批量操作因以不同顺序提供相同 ID 而死锁。
        for id in &ids {
            self.lock_manageable_user_in_txn(actor, &transaction, *id)
                .await?;
        }
        let versions = self
            .invalidate_sessions_for_tenant_in_txn(&transaction, tenant_id, &ids)
            .await?;
        let affected = self
            .user_repo
            .delete_many(&transaction, tenant_id, &ids)
            .await?;
        if affected != ids.len() as u64 {
            return Err(AppError::NotFound("用户不存在".into()));
        }
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_user_versions(tenant_id, &versions)
            .await?;
        Ok(affected)
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if id == actor.user_id {
            return Err(AppError::Authorization("禁止删除自己".into()));
        }
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, tenant_id)
            .await?;
        self.lock_manageable_user_in_txn(actor, &transaction, id)
            .await?;
        let versions = self
            .invalidate_sessions_for_tenant_in_txn(&transaction, tenant_id, &[id])
            .await?;
        let affected = self
            .user_repo
            .delete_many(&transaction, tenant_id, &[id])
            .await?;
        if affected != 1 {
            return Err(AppError::NotFound("用户不存在".into()));
        }
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_user_versions(tenant_id, &versions)
            .await?;
        Ok(())
    }

    pub(super) async fn lock_manageable_user_in_txn(
        &self,
        actor: &ActorContext,
        transaction: &DatabaseTransaction,
        id: i64,
    ) -> AppResult<user::Model> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let scope = actor.data_scope_context();
        let user = self
            .user_repo
            .find_by_id_with_data_scope_for_update(transaction, tenant_id, id, &scope)
            .await?
            .ok_or_else(|| AppError::Authorization("无权访问该用户数据".into()))?;
        if self
            .role_repo
            .user_has_super_role_in_txn(transaction, tenant_id, id)
            .await?
        {
            return Err(AppError::Authorization("禁止操作超级管理员".into()));
        }
        Ok(user)
    }
}

fn normalize_ids(ids: &mut Vec<i64>) {
    ids.sort_unstable();
    ids.dedup();
}

fn validate_manageable_status(status: &str) -> AppResult<()> {
    if matches!(
        status,
        user::Model::STATUS_NORMAL | user::Model::STATUS_DISABLED
    ) {
        Ok(())
    } else {
        Err(AppError::Validation("无效的用户状态".into()))
    }
}
