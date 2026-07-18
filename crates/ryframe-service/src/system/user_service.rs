mod commands;
mod password_reset;
mod queries;
mod roles;

use ryframe_common::{AppError, AppResult};
use ryframe_core::{LoggedRepo, RedisClient, repository::PageQuery};
use ryframe_db::DatabaseCluster;
use ryframe_db::{
    DeptRepository, RoleRepository, UserRepository,
    entities::{password_reset_request, role, user},
};
use serde::Serialize;
use utoipa::ToSchema;

pub const USER_STATUS_NORMAL: &str = user::Model::STATUS_NORMAL;

#[derive(Debug, Serialize, ToSchema)]
pub struct UserVo {
    pub id: String,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub status: String,
    pub dept_id: Option<String>,
    pub dept_name: Option<String>,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<user::Model> for UserVo {
    fn from(user: user::Model) -> Self {
        Self {
            id: user.id.to_string(),
            username: user.username,
            nickname: user.nickname,
            email: user.email,
            phone: user.phone,
            avatar: user.avatar,
            status: user.status,
            dept_id: user.dept_id.map(|id| id.to_string()),
            dept_name: None,
            remark: user.remark,
            created_at: user.created_at,
        }
    }
}

#[derive(Debug)]
pub struct PasswordResetRequestOutcome {
    pub request: password_reset_request::Model,
    pub token: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserDetailVo {
    #[serde(flatten)]
    pub user: UserVo,
    pub roles: Vec<RoleBriefVo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RoleBriefVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub is_super: i8,
}

impl From<role::Model> for RoleBriefVo {
    fn from(role: role::Model) -> Self {
        Self {
            id: role.id.to_string(),
            name: role.name,
            code: role.code,
            is_super: role.is_super,
        }
    }
}

pub struct UserService {
    db: DatabaseCluster,
    user_repo: LoggedRepo<UserRepository>,
    role_repo: LoggedRepo<RoleRepository>,
    dept_repo: LoggedRepo<DeptRepository>,
}

pub struct CreateUserParams<'a> {
    pub username: &'a str,
    pub nickname: &'a str,
    pub email: &'a str,
    pub phone: &'a str,
    pub dept_id: Option<i64>,
    pub role_ids: Vec<i64>,
}

pub struct UpdateUserParams<'a> {
    pub id: i64,
    pub nickname: &'a str,
    pub email: &'a str,
    pub phone: &'a str,
    pub dept_id: Option<i64>,
}

#[derive(Debug)]
pub struct UserListParams {
    pub page: PageQuery,
    pub username: Option<String>,
    pub phone: Option<String>,
    pub status: Option<String>,
    pub dept_id: Option<i64>,
}

impl UserListParams {
    pub fn page_only(page: PageQuery) -> Self {
        Self {
            page,
            username: None,
            phone: None,
            status: None,
            dept_id: None,
        }
    }
}

impl UserService {
    pub fn new(db: DatabaseCluster, _redis: Option<RedisClient>) -> Self {
        Self {
            db,
            user_repo: LoggedRepo::new(UserRepository),
            role_repo: LoggedRepo::new(RoleRepository),
            dept_repo: LoggedRepo::new(DeptRepository),
        }
    }

    pub(super) async fn invalidate_sessions_for_tenant(
        &self,
        tenant_id: &str,
        user_ids: &[i64],
    ) -> AppResult<()> {
        let mut user_ids = user_ids.to_vec();
        user_ids.sort_unstable();
        user_ids.dedup();
        if user_ids.is_empty() {
            return Ok(());
        }

        let affected = self
            .user_repo
            .increment_auth_versions(self.db.write(), tenant_id, &user_ids)
            .await?;
        if affected == 0 {
            return Err(AppError::NotFound("用户不存在".into()));
        }
        Ok(())
    }
}
