use ryframe_auth::password;
use ryframe_core::{
    Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::{DatabaseCluster, ReadConsistency};
use ryframe_db::{
    DeptRepository, FileRepository, PermissionRepository, RoleRepository, UserRepository,
    entities::{sys_file, user},
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::{ActiveModelTrait, TransactionTrait};
use serde::Serialize;

use crate::AuthorizationCache;

const AVATAR_CLEANUP_GRACE_MINUTES: i64 = 5;

/// 用户个人信息响应
#[derive(Debug, Clone, Serialize)]
pub struct UserProfileResponse {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
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

/// 个人中心服务
pub struct ProfileService {
    db: DatabaseCluster,
    user_repo: UserRepository,
    role_repo: RoleRepository,
    perm_repo: PermissionRepository,
    dept_repo: DeptRepository,
    authorization_cache: AuthorizationCache,
}

impl ProfileService {
    pub fn new(db: DatabaseCluster, authorization_cache: AuthorizationCache) -> Self {
        Self {
            db,
            user_repo: UserRepository,
            role_repo: RoleRepository,
            perm_repo: PermissionRepository,
            dept_repo: DeptRepository,
            authorization_cache,
        }
    }
    /// 获取当前用户个人信息
    pub async fn get_profile(&self, actor: &ActorContext) -> AppResult<UserProfileResponse> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        // 查询用户信息
        let user = self
            .user_repo
            .find_by_id(&db, tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        // 查询部门名称
        let dept_name = if let Some(dept_id) = user.dept_id {
            self.dept_repo
                .find_by_id(&db, tenant_id, dept_id)
                .await?
                .map(|d| d.name)
        } else {
            None
        };

        // 查询角色和权限
        let roles = self
            .role_repo
            .find_user_roles(&db, tenant_id, user.id)
            .await?;
        let role_codes: Vec<String> = roles.iter().map(|r| r.code.clone()).collect();
        let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();
        let perms = self
            .perm_repo
            .find_role_perms(&db, tenant_id, &role_ids)
            .await?;
        let permissions: Vec<String> = perms.iter().map(|p| p.code.clone()).collect();

        Ok(UserProfileResponse {
            user_id: user.id.to_string(),
            username: user.username,
            nickname: user.nickname,
            email: user.email,
            phone: user.phone,
            avatar: user.avatar,
            preferred_locale: user.preferred_locale,
            dept_id: user.dept_id.map(|id| id.to_string()),
            dept_name,
            status: user.status,
            remark: user.remark,
            login_ip: user.login_ip,
            login_date: user.login_date.map(|d| d.to_rfc3339()),
            created_at: user.created_at.to_rfc3339(),
            roles: role_codes,
            permissions,
        })
    }

    /// 更新个人信息
    pub async fn update_profile(
        &self,
        actor: &ActorContext,
        nickname: String,
        email: String,
        phone: String,
        preferred_locale: Option<String>,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let mut user = self
            .user_repo
            .find_by_id_for_update(&transaction, tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        user.nickname = nickname;
        user.email = email;
        user.phone = phone;
        user.preferred_locale = normalize_preferred_locale(preferred_locale)?;
        user.fill_on_update(&FillContext::new())?;

        user::ActiveModel::from(user)
            .reset_all()
            .update(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        crate::commit_current_audit(transaction).await?;
        Ok(())
    }

    /// 修改密码
    pub async fn change_password(
        &self,
        actor: &ActorContext,
        old_password: &str,
        new_password: &str,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let mut user = self
            .user_repo
            .find_by_id_for_update(&transaction, tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        // 验证旧密码
        if !password::verify(old_password, &user.password_hash)? {
            return Err(AppError::Validation("旧密码不正确".into()));
        }
        if old_password == new_password {
            return Err(AppError::Validation("新密码不能与旧密码相同".into()));
        }

        password::validate_complexity(new_password)?;
        let new_hash = password::hash(new_password)?;
        user.password_hash = new_hash;
        user.fill_on_update(&FillContext::new())?;

        user::ActiveModel::from(user)
            .reset_all()
            .update(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let versions = self
            .authorization_cache
            .increment_user_versions_in_transaction(&transaction, tenant_id, &[actor.user_id])
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_user_versions(tenant_id, &versions)
            .await?;
        Ok(())
    }

    /// 更新头像并维护可回收的文件关联。
    pub async fn update_avatar(
        &self,
        actor: &ActorContext,
        avatar_url: String,
        avatar_file_id: i64,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

        let result = async {
            // 先锁定用户行，让同一用户的头像替换串行执行。
            let user = self
                .user_repo
                .find_by_id_for_update(&transaction, tenant_id, actor.user_id)
                .await?
                .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
            let old_avatar_file_id = user.avatar_file_id;
            let now = FileRepository.database_utc_now(&transaction).await?;

            // 多用户可通过内容去重引用同一个文件；固定按文件 ID 锁定，避免交叉替换
            // 时产生锁顺序反转。
            let mut file_ids = vec![avatar_file_id];
            if let Some(old_file_id) = old_avatar_file_id.filter(|id| *id != avatar_file_id) {
                file_ids.push(old_file_id);
            }
            file_ids.sort_unstable();
            file_ids.dedup();

            let mut avatar_file = None;
            for file_id in file_ids {
                let file = FileRepository
                    .find_by_id_any_status_for_update(&transaction, tenant_id, file_id)
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
            if avatar_file.upload_status == sys_file::Model::UPLOAD_STATUS_CLEANUP {
                if !FileRepository
                    .restore_avatar_file_for_reference_in_txn(
                        &transaction,
                        tenant_id,
                        avatar_file_id,
                        now,
                    )
                    .await?
                {
                    return Err(AppError::NotFound("头像文件已进入最终回收阶段".into()));
                }
            } else if avatar_file.upload_status != sys_file::Model::UPLOAD_STATUS_READY
                || avatar_file.del_flag != sys_file::Model::DEL_FLAG_NORMAL
            {
                return Err(AppError::Validation("头像文件尚未完成上传".into()));
            }

            self.user_repo
                .update_avatar_in_txn(
                    &transaction,
                    tenant_id,
                    actor.user_id,
                    avatar_url,
                    avatar_file_id,
                    now,
                )
                .await?;

            if let Some(old_file_id) = old_avatar_file_id.filter(|id| *id != avatar_file_id) {
                let references = self
                    .user_repo
                    .count_avatar_file_references_in_txn(&transaction, tenant_id, old_file_id)
                    .await?;
                if references == 0 {
                    FileRepository
                        .mark_avatar_orphan_for_cleanup_in_txn(
                            &transaction,
                            tenant_id,
                            old_file_id,
                            now,
                            now + chrono::Duration::minutes(AVATAR_CLEANUP_GRACE_MINUTES),
                        )
                        .await?;
                }
            }

            Ok(())
        }
        .await;

        match result {
            Ok(()) => crate::commit_current_audit(transaction).await,
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    /// 在头像关联事务失败后，将新上传但未被任何用户引用的文件纳入延迟回收。
    pub async fn schedule_unreferenced_avatar_cleanup(
        &self,
        actor: &ActorContext,
        avatar_file_id: i64,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let result: AppResult<bool> = async {
            let now = FileRepository.database_utc_now(&transaction).await?;
            let Some(file) = FileRepository
                .find_by_id_any_status_for_update(&transaction, tenant_id, avatar_file_id)
                .await?
            else {
                return Ok(false);
            };
            if file.bucket != "avatar" {
                return Err(AppError::Validation("只能清理头像专用文件".into()));
            }
            let references = self
                .user_repo
                .count_avatar_file_references_in_txn(&transaction, tenant_id, avatar_file_id)
                .await?;
            if references == 0 {
                return FileRepository
                    .mark_avatar_orphan_for_cleanup_in_txn(
                        &transaction,
                        tenant_id,
                        avatar_file_id,
                        now,
                        now + chrono::Duration::minutes(AVATAR_CLEANUP_GRACE_MINUTES),
                    )
                    .await;
            }
            Ok(false)
        }
        .await;
        match result {
            Ok(true) => crate::commit_current_audit(transaction).await,
            Ok(false) => transaction
                .rollback()
                .await
                .map_err(|error| AppError::Database(error.to_string())),
            Err(error) => {
                if let Err(rollback_error) = transaction.rollback().await {
                    tracing::error!(%rollback_error, "头像延迟回收事务回滚失败");
                }
                Err(error)
            }
        }
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
