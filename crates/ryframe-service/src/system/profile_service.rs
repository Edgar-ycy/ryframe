use ryframe_auth::password;
use ryframe_common::{AppError, AppResult};
use ryframe_core::Repository;
use ryframe_db::{UserRepository, user, dept};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait};
use serde::{Deserialize, Serialize};
use validator::Validate;

// ── DTO 定义 ──

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

/// 更新个人信息请求
#[derive(Debug, Deserialize, Validate)]
pub struct UpdateProfileRequest {
    #[validate(length(min = 1, max = 64, message = "昵称长度为1-64个字符"))]
    pub nickname: String,
    #[validate(email(message = "邮箱格式不正确"))]
    pub email: Option<String>,
    #[validate(length(max = 32, message = "手机号最多32个字符"))]
    pub phone: Option<String>,
    pub sex: Option<String>,
}

/// 修改密码请求
#[derive(Debug, Deserialize, Validate)]
pub struct ChangePasswordRequest {
    #[validate(length(min = 6, max = 100, message = "密码长度为6-100个字符"))]
    pub old_password: String,
    #[validate(length(min = 6, max = 100, message = "新密码长度为6-100个字符"))]
    pub new_password: String,
}

// ── 服务实现 ──

/// 个人中心服务
pub struct ProfileServiceImpl {
    pub user_repo: UserRepository,
}

impl ProfileServiceImpl {
    /// 获取当前用户个人信息
    pub async fn get_profile(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
    ) -> AppResult<UserProfileResponse> {
        // 查询用户信息
        let user = self.user_repo.find_by_id(db, user_id).await?
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

        // TODO: 从 JWT Claims 中获取 roles 和 permissions
        // 这里简化处理，实际需要查询数据库
        let roles = vec![];
        let permissions = vec![];

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
            roles,
            permissions,
        })
    }

    /// 更新个人信息
    pub async fn update_profile(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
        req: UpdateProfileRequest,
    ) -> AppResult<()> {
        let mut user = self.user_repo.find_by_id(db, user_id).await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        user.nickname = req.nickname;
        user.email = req.email.unwrap_or_default();
        user.phone = req.phone.unwrap_or_default();

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
        req: ChangePasswordRequest,
    ) -> AppResult<()> {
        let mut user = self.user_repo.find_by_id(db, user_id).await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        // 验证旧密码
        if !password::verify(&req.old_password, &user.password_hash)? {
            return Err(AppError::Validation("旧密码不正确".into()));
        }

        // 哈希新密码
        let new_hash = password::hash(&req.new_password)?;
        user.password_hash = new_hash;

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
        let mut user = self.user_repo.find_by_id(db, user_id).await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        user.avatar = Some(avatar_url);

        let active_model: user::ActiveModel = user.into();
        active_model
            .update(db)
            .await
            .map_err(|e| AppError::Database(format!("更新头像失败: {}", e)))?;

        Ok(())
    }
}
