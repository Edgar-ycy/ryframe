use ryframe_auth::password;
use ryframe_common::{ActorContext, AppError, AppResult};
use ryframe_core::{
    LoggedRepo, Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_db::DatabaseCluster;
use ryframe_db::{DeptRepository, PermissionRepository, RoleRepository, UserRepository};
use serde::Serialize;
use utoipa::ToSchema;

/// 用户个人信息响应
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UserProfileResponse {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub user_id: String,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
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
    user_repo: LoggedRepo<UserRepository>,
    role_repo: LoggedRepo<RoleRepository>,
    perm_repo: LoggedRepo<PermissionRepository>,
    dept_repo: LoggedRepo<DeptRepository>,
}

impl ProfileService {
    pub fn new(db: DatabaseCluster) -> Self {
        Self {
            db,
            user_repo: LoggedRepo::new(UserRepository),
            role_repo: LoggedRepo::new(RoleRepository),
            perm_repo: LoggedRepo::new(PermissionRepository),
            dept_repo: LoggedRepo::new(DeptRepository),
        }
    }
    /// 获取当前用户个人信息
    pub async fn get_profile(&self, actor: &ActorContext) -> AppResult<UserProfileResponse> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.read();
        // 查询用户信息
        let user = self
            .user_repo
            .find_by_id(db, tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        // 查询部门名称
        let dept_name = if let Some(dept_id) = user.dept_id {
            self.dept_repo
                .find_by_id(db, tenant_id, dept_id)
                .await?
                .map(|d| d.name)
        } else {
            None
        };

        // 查询角色和权限
        let roles = self
            .role_repo
            .find_user_roles(db, tenant_id, user.id)
            .await?;
        let role_codes: Vec<String> = roles.iter().map(|r| r.code.clone()).collect();
        let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();
        let perms = self
            .perm_repo
            .find_role_perms(db, tenant_id, &role_ids)
            .await?;
        let permissions: Vec<String> = perms.iter().map(|p| p.code.clone()).collect();

        Ok(UserProfileResponse {
            user_id: user.id.to_string(),
            username: user.username,
            nickname: user.nickname,
            email: user.email,
            phone: user.phone,
            avatar: user.avatar,
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
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        let mut user = self
            .user_repo
            .find_by_id(db, tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        user.nickname = nickname;
        user.email = email;
        user.phone = phone;
        user.fill_on_update(&FillContext::new())?;

        self.user_repo.update(db, tenant_id, user).await?;

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
        let db = self.db.write();
        let mut user = self
            .user_repo
            .find_by_id(db, tenant_id, actor.user_id)
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
        user.auth_version = user.auth_version.saturating_add(1);
        user.fill_on_update(&FillContext::new())?;

        self.user_repo.update(db, tenant_id, user).await?;

        Ok(())
    }

    /// 更新头像
    pub async fn update_avatar(&self, actor: &ActorContext, avatar_url: String) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let db = self.db.write();
        // 读取当前头像路径用于后续清理
        let mut user = self
            .user_repo
            .find_by_id(db, tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        let old_avatar = user.avatar.clone();
        user.avatar = Some(avatar_url);
        user.fill_on_update(&FillContext::new())?;
        self.user_repo.update(db, tenant_id, user).await?;

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
