use ryframe_auth::password;
use ryframe_common::{ActorContext, AppError, AppResult, utils::snowflake};
use ryframe_core::{
    Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::{PasswordResetRequestRepository, entities::password_reset_request};
use sea_orm::{
    ColumnTrait, EntityTrait, ExprTrait, QueryFilter, QuerySelect, TransactionTrait,
    sea_query::{Expr, LockType},
};
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
        let database_now = PasswordResetRequestRepository
            .database_utc_now(self.db.write())
            .await?;
        let mut request = password_reset_request::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            target_user_id,
            requested_by: actor.user_id,
            reason: reason.to_owned(),
            token_hash: password::hash(&token)?,
            expires_at: database_now + chrono::Duration::hours(24),
            completed_at: None,
            request_ip,
            status: password_reset_request::Model::STATUS_PENDING.into(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        request.fill_on_insert(&FillContext::new())?;
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

        let reset_request = PasswordResetRequestRepository
            .find_by_id(self.db.write(), tenant_id, request_id)
            .await?
            .ok_or_else(|| AppError::NotFound("密码重置请求不存在".into()))?;
        if reset_request.status != password_reset_request::Model::STATUS_PENDING
            || reset_request.completed_at.is_some()
        {
            return Err(AppError::Validation("密码重置请求已处理".into()));
        }
        let evaluated_at = PasswordResetRequestRepository
            .database_utc_now(self.db.write())
            .await?;
        if reset_request.expires_at <= evaluated_at {
            if PasswordResetRequestRepository
                .expire_pending(self.db.write(), tenant_id, request_id, evaluated_at)
                .await?
            {
                return Err(AppError::Validation("密码重置请求已过期".into()));
            }
            return Err(AppError::Validation("密码重置请求已处理".into()));
        }
        if !password::verify(token, &reset_request.token_hash)? {
            return Err(AppError::Authentication("密码重置令牌无效".into()));
        }

        let target_user = self
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
        let password_hash = password::hash(new_password)?;
        let observed_status = target_user.status.clone();
        let next_status = if matches!(
            observed_status.as_str(),
            ryframe_db::entities::user::Model::STATUS_PENDING_ACTIVATION
                | ryframe_db::entities::user::Model::STATUS_MUST_RESET_PASSWORD
        ) {
            ryframe_db::entities::user::Model::STATUS_NORMAL.to_owned()
        } else {
            observed_status.clone()
        };
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        let completed_at = PasswordResetRequestRepository
            .database_utc_now(&transaction)
            .await?;
        let consumed = PasswordResetRequestRepository
            .complete_pending_in_txn(&transaction, tenant_id, request_id, completed_at)
            .await?;
        if !consumed {
            return Err(AppError::Validation("密码重置请求已处理或已过期".into()));
        }

        let current_user = ryframe_db::entities::user::Entity::find_by_id(user_id)
            .filter(ryframe_db::entities::user::Column::TenantId.eq(tenant_id))
            .filter(
                ryframe_db::entities::user::Column::DelFlag
                    .eq(ryframe_db::entities::user::Model::DEL_FLAG_NORMAL),
            )
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        if current_user.auth_version != target_user.auth_version
            || current_user.status != target_user.status
        {
            return Err(AppError::Conflict(
                "用户认证状态已发生变化，请重新发起密码重置".into(),
            ));
        }
        let current_roles = self
            .role_repo
            .find_user_roles_all_status(&transaction, tenant_id, user_id)
            .await?;
        if current_roles.iter().any(|role| role.is_super == 1) {
            return Err(AppError::Authorization("禁止操作超级管理员".into()));
        }

        // Update only authentication fields. Guarding the observed status prevents a
        // concurrent administrator action or password change from being overwritten.
        let update_result =
            guarded_password_update(&target_user, password_hash, next_status, completed_at)
                .exec(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        if update_result.rows_affected != 1 {
            return Err(AppError::Conflict(
                "用户认证状态已发生变化，请重新发起密码重置".into(),
            ));
        }
        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(format!("提交事务失败: {error}")))?;
        Ok(user_id)
    }
}

fn guarded_password_update(
    target_user: &ryframe_db::entities::user::Model,
    password_hash: String,
    next_status: String,
    updated_at: chrono::DateTime<chrono::Utc>,
) -> sea_orm::UpdateMany<ryframe_db::entities::user::Entity> {
    ryframe_db::entities::user::Entity::update_many()
        .col_expr(
            ryframe_db::entities::user::Column::PasswordHash,
            Expr::value(password_hash),
        )
        .col_expr(
            ryframe_db::entities::user::Column::AuthVersion,
            Expr::col(ryframe_db::entities::user::Column::AuthVersion).add(1),
        )
        .col_expr(
            ryframe_db::entities::user::Column::Status,
            Expr::value(next_status),
        )
        .col_expr(
            ryframe_db::entities::user::Column::UpdatedAt,
            Expr::value(updated_at),
        )
        .filter(ryframe_db::entities::user::Column::Id.eq(target_user.id))
        .filter(ryframe_db::entities::user::Column::TenantId.eq(target_user.tenant_id.as_str()))
        .filter(
            ryframe_db::entities::user::Column::DelFlag
                .eq(ryframe_db::entities::user::Model::DEL_FLAG_NORMAL),
        )
        .filter(ryframe_db::entities::user::Column::Status.eq(target_user.status.as_str()))
        .filter(ryframe_db::entities::user::Column::AuthVersion.eq(target_user.auth_version))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use sea_orm::{DbBackend, QueryTrait};

    use super::guarded_password_update;

    #[test]
    fn password_update_uses_status_and_auth_version_cas() {
        let now = Utc::now();
        let user = ryframe_db::entities::user::Model {
            id: 42,
            tenant_id: "tenant-a".into(),
            username: "user".into(),
            password_hash: "old-hash".into(),
            nickname: "User".into(),
            email: String::new(),
            phone: String::new(),
            avatar: None,
            status: ryframe_db::entities::user::Model::STATUS_NORMAL.into(),
            auth_version: 7,
            dept_id: None,
            remark: None,
            login_ip: None,
            login_date: None,
            del_flag: ryframe_db::entities::user::Model::DEL_FLAG_NORMAL.into(),
            created_at: now,
            updated_at: now,
        };

        let statement = guarded_password_update(
            &user,
            "new-hash".into(),
            ryframe_db::entities::user::Model::STATUS_NORMAL.into(),
            now,
        )
        .build(DbBackend::MySql);

        assert_eq!(statement.sql.matches("`auth_version` =").count(), 2);
        assert_eq!(statement.sql.matches("`status` =").count(), 2);
    }
}
