use ryframe_auth::password;
use ryframe_common::{AppError, AppResult};
use ryframe_core::{
    LoggedRepo, Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::{PermissionRepository, RoleRepository, UserRepository, dept, user};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait};
use serde::Serialize;

/// 用户个人信息响应
#[derive(Debug, Clone, Serialize)]
pub struct UserProfileResponse {
    pub user_id: i64,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub dept_id: Option<i64>,
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
pub struct ProfileServiceImpl {
    pub user_repo: LoggedRepo<UserRepository>,
    pub role_repo: LoggedRepo<RoleRepository>,
    pub perm_repo: LoggedRepo<PermissionRepository>,
}

impl ProfileServiceImpl {
    /// 获取当前用户个人信息
    pub async fn get_profile(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
    ) -> AppResult<UserProfileResponse> {
        // 查询用户信息
        let user = self
            .user_repo
            .find_by_id(db, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        // 查询部门名称
        let dept_name = if let Some(dept_id) = user.dept_id {
            dept::Entity::find_by_id(dept_id)
                .one(db)
                .await
                .map_err(|e| AppError::Database(format!("查询部门失败: {}", e)))?
                .map(|d| d.name)
        } else {
            None
        };

        // 查询角色和权限
        let roles = self.role_repo.find_user_roles(db, user.id).await?;
        let role_codes: Vec<String> = roles.iter().map(|r| r.code.clone()).collect();
        let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();
        let perms = self.perm_repo.find_role_perms(db, &role_ids).await?;
        let permissions: Vec<String> = perms.iter().map(|p| p.code.clone()).collect();

        Ok(UserProfileResponse {
            user_id: user.id,
            username: user.username,
            nickname: user.nickname,
            email: user.email,
            phone: user.phone,
            avatar: user.avatar,
            dept_id: user.dept_id,
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
        db: &DatabaseConnection,
        user_id: i64,
        nickname: String,
        email: String,
        phone: String,
    ) -> AppResult<()> {
        let mut user = self
            .user_repo
            .find_by_id(db, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        user.nickname = nickname;
        user.email = email;
        user.phone = phone;
        user.fill_on_update(&FillContext::new());

        // 更新用户信息
        let active_model: user::ActiveModel = user.into();
        active_model
            .update(db)
            .await
            .map_err(|e| AppError::Database(format!("更新用户信息失败: {}", e)))?;

        Ok(())
    }

    /// 修改密码
    pub async fn change_password(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
        old_password: &str,
        new_password: &str,
    ) -> AppResult<()> {
        let mut user = self
            .user_repo
            .find_by_id(db, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        // 验证旧密码
        if !password::verify(old_password, &user.password_hash)? {
            return Err(AppError::Validation("旧密码不正确".into()));
        }

        // 哈希新密码
        let new_hash = password::hash(new_password)?;
        user.password_hash = new_hash;
        user.fill_on_update(&FillContext::new());

        // 更新密码
        let active_model: user::ActiveModel = user.into();
        active_model
            .update(db)
            .await
            .map_err(|e| AppError::Database(format!("修改密码失败: {}", e)))?;

        Ok(())
    }

    /// 更新头像
    pub async fn update_avatar(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
        avatar_url: String,
    ) -> AppResult<()> {
        let mut user = self
            .user_repo
            .find_by_id(db, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        // 记录旧头像路径，用于清理
        let old_avatar = user.avatar.clone();

        user.avatar = Some(avatar_url);
        user.fill_on_update(&FillContext::new());

        let active_model: user::ActiveModel = user.into();
        active_model
            .update(db)
            .await
            .map_err(|e| AppError::Database(format!("更新头像失败: {}", e)))?;

        // 清理旧头像文件（仅清理本地上传的文件）
        if let Some(old_path) = old_avatar
            && (old_path.starts_with("/upload/") || old_path.starts_with("upload/"))
        {
            let file_path =
                std::path::PathBuf::from("upload").join(old_path.trim_start_matches('/'));
            if file_path.exists()
                && let Err(e) = std::fs::remove_file(&file_path)
            {
                tracing::warn!("清理旧头像文件失败: {}: {}", file_path.display(), e);
            }
        }

        Ok(())
    }
}
