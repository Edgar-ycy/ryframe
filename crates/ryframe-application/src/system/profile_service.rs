use std::sync::Arc;

use ryframe_auth::password;
use ryframe_kernel::{ActorContext, AppError, AppResult};
use serde::Serialize;

use crate::{AuthorizationCache, ProfileAvatarState, ProfilePersistencePort, ProfileTransaction};

const AVATAR_CLEANUP_GRACE_MINUTES: i64 = 5;

/// 用户个人信息响应。
#[derive(Debug, Clone, Serialize)]
pub struct UserProfileResponse {
    /// ID 使用字符串，避免超出 JavaScript 安全整数范围。
    pub user_id: String,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub preferred_locale: Option<String>,
    pub dept_id: Option<String>,
    pub dept_name: Option<String>,
    pub status: String,
    pub remark: Option<String>,
    pub login_ip: Option<String>,
    pub login_date: Option<String>,
    pub created_at: String,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
}

/// 个人中心用例。
pub struct ProfileService {
    persistence: Arc<dyn ProfilePersistencePort>,
    authorization_cache: AuthorizationCache,
}

impl ProfileService {
    pub fn new(
        persistence: Arc<dyn ProfilePersistencePort>,
        authorization_cache: AuthorizationCache,
    ) -> Self {
        Self {
            persistence,
            authorization_cache,
        }
    }

    /// 获取当前用户个人信息。
    pub async fn get_profile(&self, actor: &ActorContext) -> AppResult<UserProfileResponse> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let profile = self
            .persistence
            .find_profile(tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        Ok(UserProfileResponse {
            user_id: profile.user_id.to_string(),
            username: profile.username,
            nickname: profile.nickname,
            email: profile.email,
            phone: profile.phone,
            avatar: profile.avatar,
            preferred_locale: profile.preferred_locale,
            dept_id: profile.dept_id.map(|id| id.to_string()),
            dept_name: profile.dept_name,
            status: profile.status,
            remark: profile.remark,
            login_ip: profile.login_ip,
            login_date: profile.login_date.map(|date| date.to_rfc3339()),
            created_at: profile.created_at.to_rfc3339(),
            roles: profile.roles,
            permissions: profile.permissions,
        })
    }

    /// 更新个人信息。
    pub async fn update_profile(
        &self,
        actor: &ActorContext,
        nickname: String,
        email: String,
        phone: String,
        preferred_locale: Option<String>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction
            .update_profile(
                tenant_id,
                actor.user_id,
                nickname,
                email,
                phone,
                normalize_preferred_locale(preferred_locale)?,
            )
            .await?;
        transaction.commit().await
    }

    /// 修改密码。
    pub async fn change_password(
        &self,
        actor: &ActorContext,
        old_password: &str,
        new_password: &str,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_tenant(tenant_id).await?;
        let user = transaction
            .find_user_for_update(tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        if !password::verify(old_password, &user.password_hash)? {
            return Err(AppError::Validation("旧密码不正确".into()));
        }
        if old_password == new_password {
            return Err(AppError::Validation("新密码不能与旧密码相同".into()));
        }
        password::validate_complexity(new_password)?;
        transaction
            .update_password(tenant_id, actor.user_id, password::hash(new_password)?)
            .await?;
        let versions = transaction
            .increment_user_authorization_version(tenant_id, actor.user_id)
            .await?;
        transaction.commit().await?;
        self.authorization_cache
            .sync_user_versions(tenant_id, &versions)
            .await
    }

    /// 更新头像并维护可回收的文件关联。
    pub async fn update_avatar(
        &self,
        actor: &ActorContext,
        avatar_url: String,
        avatar_file_id: i64,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        let result = self
            .update_avatar_in_transaction(
                transaction.as_ref(),
                tenant_id,
                actor.user_id,
                avatar_url,
                avatar_file_id,
            )
            .await;
        match result {
            Ok(()) => transaction.commit().await,
            Err(error) => {
                if let Err(rollback_error) = transaction.rollback().await {
                    tracing::error!(%rollback_error, "头像关联事务回滚失败");
                }
                Err(error)
            }
        }
    }

    async fn update_avatar_in_transaction(
        &self,
        transaction: &dyn ProfileTransaction,
        tenant_id: &str,
        user_id: i64,
        avatar_url: String,
        avatar_file_id: i64,
    ) -> AppResult<()> {
        let user = transaction
            .find_user_for_update(tenant_id, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        let old_avatar_file_id = user.avatar_file_id;
        let now = transaction.database_now().await?;

        let mut file_ids = vec![avatar_file_id];
        if let Some(old_file_id) = old_avatar_file_id.filter(|id| *id != avatar_file_id) {
            file_ids.push(old_file_id);
        }
        file_ids.sort_unstable();
        file_ids.dedup();

        let mut avatar_file = None;
        for file_id in file_ids {
            let file = transaction
                .find_avatar_file_for_update(tenant_id, file_id)
                .await?;
            if file_id == avatar_file_id {
                avatar_file = file;
            }
        }
        let avatar_file =
            avatar_file.ok_or_else(|| AppError::NotFound("头像文件不存在或已被回收".into()))?;
        if avatar_file.bucket != "avatar" {
            return Err(AppError::Validation("只能关联头像专用文件".into()));
        }
        match avatar_file.state {
            ProfileAvatarState::Ready => {}
            ProfileAvatarState::Cleanup => {
                if !transaction
                    .restore_avatar_file(tenant_id, avatar_file_id, now)
                    .await?
                {
                    return Err(AppError::NotFound("头像文件已进入最终回收阶段".into()));
                }
            }
            ProfileAvatarState::Unavailable => {
                return Err(AppError::Validation("头像文件尚未完成上传".into()));
            }
        }

        transaction
            .update_avatar(tenant_id, user_id, avatar_url, avatar_file_id, now)
            .await?;

        if let Some(old_file_id) = old_avatar_file_id.filter(|id| *id != avatar_file_id)
            && transaction
                .count_avatar_references(tenant_id, old_file_id)
                .await?
                == 0
        {
            transaction
                .mark_avatar_orphan(
                    tenant_id,
                    old_file_id,
                    now,
                    now + chrono::Duration::minutes(AVATAR_CLEANUP_GRACE_MINUTES),
                )
                .await?;
        }
        Ok(())
    }

    /// 在头像关联事务失败后，将未被引用的文件纳入延迟回收。
    pub async fn schedule_unreferenced_avatar_cleanup(
        &self,
        actor: &ActorContext,
        avatar_file_id: i64,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        let result = self
            .schedule_avatar_cleanup_in_transaction(transaction.as_ref(), tenant_id, avatar_file_id)
            .await;
        match result {
            Ok(true) => transaction.commit().await,
            Ok(false) => transaction.rollback().await,
            Err(error) => {
                if let Err(rollback_error) = transaction.rollback().await {
                    tracing::error!(%rollback_error, "头像延迟回收事务回滚失败");
                }
                Err(error)
            }
        }
    }

    async fn schedule_avatar_cleanup_in_transaction(
        &self,
        transaction: &dyn ProfileTransaction,
        tenant_id: &str,
        avatar_file_id: i64,
    ) -> AppResult<bool> {
        let now = transaction.database_now().await?;
        let Some(file) = transaction
            .find_avatar_file_for_update(tenant_id, avatar_file_id)
            .await?
        else {
            return Ok(false);
        };
        if file.bucket != "avatar" {
            return Err(AppError::Validation("只能清理头像专用文件".into()));
        }
        if transaction
            .count_avatar_references(tenant_id, avatar_file_id)
            .await?
            != 0
        {
            return Ok(false);
        }
        transaction
            .mark_avatar_orphan(
                tenant_id,
                avatar_file_id,
                now,
                now + chrono::Duration::minutes(AVATAR_CLEANUP_GRACE_MINUTES),
            )
            .await
    }
}

fn normalize_preferred_locale(locale: Option<String>) -> AppResult<Option<String>> {
    match locale.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some("zh-CN") => Ok(Some("zh-CN".into())),
        Some("en-US") => Ok(Some("en-US".into())),
        Some(_) => Err(AppError::Validation(
            "preferred_locale 只能是 zh-CN、en-US 或空值".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_preferred_locale;

    #[test]
    fn preferred_locale_accepts_only_supported_values() {
        assert_eq!(normalize_preferred_locale(None).unwrap(), None);
        assert_eq!(normalize_preferred_locale(Some("  ".into())).unwrap(), None);
        assert_eq!(
            normalize_preferred_locale(Some("zh-CN".into())).unwrap(),
            Some("zh-CN".into())
        );
        assert!(normalize_preferred_locale(Some("zh-cn".into())).is_err());
    }
}
