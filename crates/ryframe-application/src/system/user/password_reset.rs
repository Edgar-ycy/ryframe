use ryframe_auth::password;
use ryframe_kernel::{ActorContext, AppError, AppResult};
use uuid::Uuid;

use super::{
    PasswordResetRequestOutcome, USER_STATUS_MUST_RESET_PASSWORD, USER_STATUS_NORMAL,
    USER_STATUS_PENDING_ACTIVATION, UserService,
};
use crate::ports::auth::{
    NewPasswordResetRequest, PASSWORD_RESET_STATUS_PENDING, PasswordResetTransaction,
    PasswordResetUserState,
};

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
        let database_now = self.password_resets.database_now().await?;
        let transaction = self.password_resets.begin().await?;
        let result = async {
            transaction.lock_tenant(tenant_id).await?;
            let target = transaction
                .lock_manageable_user(tenant_id, target_user_id, &actor.data_scope_context())
                .await?
                .ok_or_else(|| AppError::Authorization("无权访问该用户数据".into()))?;
            ensure_not_super(&target)?;
            transaction
                .insert_request(NewPasswordResetRequest {
                    id: crate::next_id()?,
                    tenant_id: tenant_id.to_owned(),
                    target_user_id,
                    requested_by: actor.user_id,
                    reason: reason.to_owned(),
                    token_hash: password::hash(&token)?,
                    expires_at: database_now + chrono::Duration::hours(24),
                    request_ip,
                })
                .await
        }
        .await;
        match result {
            Ok(request) => {
                transaction.commit().await?;
                Ok(PasswordResetRequestOutcome { request, token })
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn complete_password_reset_request(
        &self,
        tenant_id: &str,
        request_id: i64,
        token: &str,
        new_password: &str,
    ) -> AppResult<i64> {
        crate::enforce_tenant_scope(tenant_id)?;
        let token = token.trim();
        if token.is_empty() {
            return Err(AppError::Validation("密码重置令牌不能为空".into()));
        }
        password::validate_complexity(new_password)?;

        let reset_request = self
            .password_resets
            .find_request(tenant_id, request_id)
            .await?
            .ok_or_else(|| AppError::NotFound("密码重置请求不存在".into()))?;
        ensure_pending(&reset_request.status, reset_request.completed_at)?;
        let evaluated_at = self.password_resets.database_now().await?;
        // 昂贵的密码散列留在事务外执行；进入事务后仍会重新校验请求、用户和角色状态。
        let prepared = if reset_request.expires_at > evaluated_at {
            if !password::verify(token, &reset_request.token_hash)? {
                return Err(AppError::Authentication("密码重置令牌无效".into()));
            }
            let target = self
                .password_resets
                .find_user_state(tenant_id, reset_request.target_user_id)
                .await?
                .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
            ensure_not_super(&target)?;
            Some((password::hash(new_password)?, target))
        } else {
            None
        };

        let transaction = self.password_resets.begin().await?;
        let result = self
            .complete_password_reset_in_transaction(
                transaction.as_ref(),
                tenant_id,
                request_id,
                token,
                prepared,
            )
            .await;
        match result {
            Ok(PasswordResetCompletion::Expired) => {
                transaction.commit().await?;
                Err(AppError::Validation("密码重置请求已过期".into()))
            }
            Ok(PasswordResetCompletion::Completed {
                user_id,
                authorization_version,
            }) => {
                transaction.commit().await?;
                self.authorization_cache
                    .sync_user_versions(tenant_id, &[(user_id, authorization_version)])
                    .await?;
                Ok(user_id)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    async fn complete_password_reset_in_transaction(
        &self,
        transaction: &dyn PasswordResetTransaction,
        tenant_id: &str,
        request_id: i64,
        token: &str,
        prepared: Option<(String, PasswordResetUserState)>,
    ) -> AppResult<PasswordResetCompletion> {
        transaction.lock_tenant(tenant_id).await?;
        let current_request = transaction
            .lock_request(tenant_id, request_id)
            .await?
            .ok_or_else(|| AppError::NotFound("密码重置请求不存在".into()))?;
        let completed_at = transaction.database_now().await?;
        ensure_pending(&current_request.status, current_request.completed_at)?;
        if current_request.expires_at <= completed_at {
            if !transaction
                .expire_pending(tenant_id, request_id, completed_at)
                .await?
            {
                return Err(AppError::Conflict("密码重置请求状态已发生变化".into()));
            }
            return Ok(PasswordResetCompletion::Expired);
        }
        if !password::verify(token, &current_request.token_hash)? {
            return Err(AppError::Authentication("密码重置令牌无效".into()));
        }

        let current_user = transaction
            .lock_user_state(tenant_id, current_request.target_user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        ensure_not_super(&current_user)?;
        let (password_hash, prepared_user) = prepared
            .ok_or_else(|| AppError::Conflict("密码重置请求状态已发生变化，请重新提交".into()))?;
        if current_request.target_user_id != prepared_user.id
            || current_user.authorization_version != prepared_user.authorization_version
            || current_user.status != prepared_user.status
        {
            return Err(AppError::Conflict(
                "用户认证状态已发生变化，请重新发起密码重置".into(),
            ));
        }
        let next_status = password_reset_next_status(&current_user.status);
        if !transaction
            .complete_pending(tenant_id, request_id, completed_at)
            .await?
        {
            return Err(AppError::Validation("密码重置请求已处理或已过期".into()));
        }
        if !transaction
            .update_password(
                tenant_id,
                &current_user,
                password_hash,
                next_status,
                completed_at,
            )
            .await?
        {
            return Err(AppError::Conflict(
                "用户认证状态已发生变化，请重新发起密码重置".into(),
            ));
        }
        let authorization_version = current_user.authorization_version.saturating_add(1);
        transaction
            .record_user_mirror_update(tenant_id, current_user.id, authorization_version)
            .await?;
        Ok(PasswordResetCompletion::Completed {
            user_id: current_user.id,
            authorization_version,
        })
    }
}

enum PasswordResetCompletion {
    Expired,
    Completed {
        user_id: i64,
        authorization_version: i32,
    },
}

fn ensure_pending(
    status: &str,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<()> {
    if status != PASSWORD_RESET_STATUS_PENDING || completed_at.is_some() {
        Err(AppError::Validation("密码重置请求已处理".into()))
    } else {
        Ok(())
    }
}

fn ensure_not_super(user: &PasswordResetUserState) -> AppResult<()> {
    if user.has_super_role {
        Err(AppError::Authorization("禁止操作超级管理员".into()))
    } else {
        Ok(())
    }
}

fn password_reset_next_status(current: &str) -> String {
    if matches!(
        current,
        USER_STATUS_PENDING_ACTIVATION | USER_STATUS_MUST_RESET_PASSWORD
    ) {
        USER_STATUS_NORMAL.to_owned()
    } else {
        current.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_state_must_be_unfinished_and_pending() {
        assert!(ensure_pending(PASSWORD_RESET_STATUS_PENDING, None).is_ok());
        assert!(ensure_pending("completed", None).is_err());
        assert!(
            ensure_pending(
                PASSWORD_RESET_STATUS_PENDING,
                Some(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH),
            )
            .is_err()
        );
    }

    #[test]
    fn password_reset_activates_only_onboarding_states() {
        assert_eq!(
            password_reset_next_status(USER_STATUS_PENDING_ACTIVATION),
            USER_STATUS_NORMAL
        );
        assert_eq!(
            password_reset_next_status(USER_STATUS_MUST_RESET_PASSWORD),
            USER_STATUS_NORMAL
        );
        assert_eq!(password_reset_next_status("1"), "1");
    }

    #[test]
    fn super_role_is_always_rejected() {
        let regular = PasswordResetUserState {
            id: 7,
            authorization_version: 2,
            status: USER_STATUS_NORMAL.to_owned(),
            has_super_role: false,
        };
        assert!(ensure_not_super(&regular).is_ok());
        assert!(
            ensure_not_super(&PasswordResetUserState {
                has_super_role: true,
                ..regular
            })
            .is_err()
        );
    }
}
