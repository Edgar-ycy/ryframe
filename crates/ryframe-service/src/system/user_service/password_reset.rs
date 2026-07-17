use ryframe_auth::password;
use ryframe_common::{ActorContext, AppError, AppResult, utils::snowflake};
use ryframe_core::{
    Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::{PasswordResetRequestRepository, entities::password_reset_request};
use sea_orm::{ActiveModelTrait, Set, TransactionTrait};
use uuid::Uuid;

use super::{PasswordResetRequestOutcome, UserService};

impl UserService {
    pub async fn request_password_reset(
        &self,
        actor: &ActorContext,
        target_user_id: i64,
        reason: &str,
        request_ip: Option<String>,
    ) -> AppResult<PasswordResetRequestOutcome> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(AppError::Validation("密码重置原因不能为空".into()));
        }
        self.ensure_user_accessible(actor, target_user_id).await?;
        self.ensure_not_super_admin_user(actor, target_user_id)
            .await?;

        let token = Uuid::new_v4().to_string();
        let mut request = password_reset_request::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: tenant_id.to_owned(),
            target_user_id,
            requested_by: actor.user_id,
            reason: reason.to_owned(),
            token_hash: password::hash(&token)?,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
            completed_at: None,
            request_ip,
            status: password_reset_request::Model::STATUS_PENDING.into(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        request.fill_on_insert(&FillContext::new());
        let request = PasswordResetRequestRepository
            .insert(self.db.write(), tenant_id, request)
            .await?;
        Ok(PasswordResetRequestOutcome { request, token })
    }

    pub async fn complete_password_reset_request(
        &self,
        tenant_id: &str,
        request_id: i64,
        token: &str,
        new_password: &str,
    ) -> AppResult<i64> {
        ryframe_core::validate_explicit_tenant(tenant_id)?;
        self.complete_password_reset(tenant_id, request_id, token, new_password)
            .await
    }

    async fn complete_password_reset(
        &self,
        tenant_id: &str,
        request_id: i64,
        token: &str,
        new_password: &str,
    ) -> AppResult<i64> {
        let token = token.trim();
        if token.is_empty() {
            return Err(AppError::Validation("密码重置令牌不能为空".into()));
        }
        password::validate_complexity(new_password)?;

        let mut reset_request = PasswordResetRequestRepository
            .find_by_id(self.db.write(), tenant_id, request_id)
            .await?
            .ok_or_else(|| AppError::NotFound("密码重置请求不存在".into()))?;
        if reset_request.status != password_reset_request::Model::STATUS_PENDING
            || reset_request.completed_at.is_some()
        {
            return Err(AppError::Validation("密码重置请求已处理".into()));
        }
        if reset_request.expires_at <= chrono::Utc::now() {
            reset_request.status = password_reset_request::Model::STATUS_EXPIRED.into();
            reset_request.fill_on_update(&FillContext::new());
            PasswordResetRequestRepository
                .update(self.db.write(), tenant_id, reset_request)
                .await?;
            return Err(AppError::Validation("密码重置请求已过期".into()));
        }
        if !password::verify(token, &reset_request.token_hash)? {
            return Err(AppError::Authentication("密码重置令牌无效".into()));
        }

        let mut target_user = self
            .user_repo
            .find_by_id(self.db.write(), tenant_id, reset_request.target_user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        let roles = self
            .role_repo
            .find_user_roles_all_status(self.db.write(), tenant_id, target_user.id)
            .await?;
        if roles.iter().any(|role| role.is_super == 1) {
            return Err(AppError::Authorization("禁止操作超级管理员".into()));
        }

        let user_id = target_user.id;
        target_user.password_hash = password::hash(new_password)?;
        target_user.auth_version = target_user.auth_version.saturating_add(1);
        if matches!(
            target_user.status.as_str(),
            ryframe_db::entities::user::Model::STATUS_PENDING_ACTIVATION
                | ryframe_db::entities::user::Model::STATUS_MUST_RESET_PASSWORD
        ) {
            target_user.status = ryframe_db::entities::user::Model::STATUS_NORMAL.into();
        }
        target_user.fill_on_update(&FillContext::new());

        reset_request.status = password_reset_request::Model::STATUS_COMPLETED.into();
        reset_request.completed_at = Some(chrono::Utc::now());
        reset_request.fill_on_update(&FillContext::new());

        let password_hash = target_user.password_hash.clone();
        let auth_version = target_user.auth_version;
        let user_status = target_user.status.clone();
        let user_updated_at = target_user.updated_at;
        let request_status = reset_request.status.clone();
        let request_completed_at = reset_request.completed_at;
        let request_updated_at = reset_request.updated_at;

        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let mut user_active: ryframe_db::entities::user::ActiveModel = target_user.into();
        user_active.password_hash = Set(password_hash);
        user_active.auth_version = Set(auth_version);
        user_active.status = Set(user_status);
        user_active.updated_at = Set(user_updated_at);
        user_active
            .update(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

        let mut request_active: password_reset_request::ActiveModel = reset_request.into();
        request_active.status = Set(request_status);
        request_active.completed_at = Set(request_completed_at);
        request_active.updated_at = Set(request_updated_at);
        request_active
            .update(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(format!("提交事务失败: {error}")))?;
        Ok(user_id)
    }
}
