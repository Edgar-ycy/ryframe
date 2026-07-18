use ryframe_auth::password;
use ryframe_common::{ActorContext, AppError, AppResult, utils::snowflake};
use ryframe_core::{
    Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::{TenantRepository, entities::user};
use sea_orm::{ActiveModelTrait, TransactionTrait};
use uuid::Uuid;

use super::{CreateUserParams, UpdateUserParams, UserService, UserVo};

impl UserService {
    pub async fn create(
        &self,
        actor: &ActorContext,
        params: CreateUserParams<'_>,
    ) -> AppResult<UserVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        TenantRepository
            .ensure_user_quota(self.db.write(), tenant_id)
            .await?;
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
            id: snowflake::next_snowflake_id(),
            tenant_id: tenant_id.to_owned(),
            username: username.to_owned(),
            password_hash: password::hash(&activation_secret)?,
            nickname: nickname.to_owned(),
            email: email.to_owned(),
            phone: phone.to_owned(),
            avatar: None,
            status: user::Model::STATUS_PENDING_ACTIVATION.into(),
            auth_version: 1,
            dept_id,
            remark: None,
            login_ip: None,
            login_date: None,
            del_flag: user::Model::DEL_FLAG_NORMAL.into(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        user.fill_on_insert(&FillContext::new());

        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
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
        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(format!("提交事务失败: {error}")))?;
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
        self.ensure_user_accessible(actor, id).await?;
        self.ensure_not_super_admin_user(actor, id).await?;
        self.validate_assignments(actor, dept_id, None).await?;
        let mut user = self
            .user_repo
            .find_by_id(self.db.write(), tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        user.nickname = nickname.to_owned();
        user.email = email.to_owned();
        user.phone = phone.to_owned();
        user.dept_id = dept_id;
        user.fill_on_update(&FillContext::new());

        let active: user::ActiveModel = user.into();
        let saved = active
            .reset_all()
            .update(self.db.write())
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        self.invalidate_sessions_for_tenant(tenant_id, &[saved.id])
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
        self.ensure_user_accessible(actor, id).await?;
        self.ensure_not_super_admin_user(actor, id).await?;
        self.user_repo
            .update_status(self.db.write(), tenant_id, id, status)
            .await?;
        self.invalidate_sessions_for_tenant(tenant_id, &[id]).await
    }

    pub async fn delete_many(&self, actor: &ActorContext, ids: &[i64]) -> AppResult<u64> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if ids.is_empty() {
            return Err(AppError::Validation("请选择要删除的用户".into()));
        }
        for id in ids {
            if *id == actor.user_id {
                return Err(AppError::Authorization("禁止删除自己".into()));
            }
            self.ensure_user_accessible(actor, *id).await?;
            self.ensure_not_super_admin_user(actor, *id).await?;
        }
        self.invalidate_sessions_for_tenant(tenant_id, ids).await?;
        let affected = self
            .user_repo
            .delete_many(self.db.write(), tenant_id, ids)
            .await?;
        Ok(affected)
    }

    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if id == actor.user_id {
            return Err(AppError::Authorization("禁止删除自己".into()));
        }
        self.ensure_user_accessible(actor, id).await?;
        self.ensure_not_super_admin_user(actor, id).await?;
        self.invalidate_sessions_for_tenant(tenant_id, &[id])
            .await?;
        self.user_repo
            .delete(self.db.write(), tenant_id, id)
            .await?;
        Ok(())
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
