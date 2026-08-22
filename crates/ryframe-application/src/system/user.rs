mod commands;
mod password_reset;
mod queries;
mod roles;

use std::sync::Arc;

use ryframe_kernel::ValidatedPageQuery;
use serde::Serialize;

use crate::{
    AuthorizationCache, AuthorizationResolver,
    ports::auth::{
        IdentityAuthorizationReadPort, PasswordResetPersistencePort, PasswordResetRequestRecord,
    },
    ports::users::{
        UserQueryReadPort, UserQueryRecord, UserQueryRoleRecord, UserWritePersistencePort,
        UserWriteRecord,
    },
};

pub use crate::ports::users::{
    USER_STATUS_DISABLED, USER_STATUS_MUST_RESET_PASSWORD, USER_STATUS_NORMAL,
    USER_STATUS_PENDING_ACTIVATION,
};
pub use commands::{normalize_ids, validate_manageable_status};
pub use password_reset::{ensure_not_super, ensure_pending, password_reset_next_status};
pub(crate) use queries::CurrentAuthorization;
pub use roles::validate_assignment_state;

#[derive(Debug, Serialize)]
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

impl From<UserWriteRecord> for UserVo {
    fn from(user: UserWriteRecord) -> Self {
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

impl From<UserQueryRecord> for UserVo {
    fn from(user: UserQueryRecord) -> Self {
        Self {
            id: user.id.to_string(),
            username: user.username,
            nickname: user.nickname,
            email: user.email,
            phone: user.phone,
            avatar: user.avatar,
            status: user.status,
            dept_id: user.dept_id.map(|id| id.to_string()),
            dept_name: user.dept_name,
            remark: user.remark,
            created_at: user.created_at,
        }
    }
}

#[derive(Debug)]
pub struct PasswordResetRequestOutcome {
    pub request: PasswordResetRequestRecord,
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct UserDetailVo {
    #[serde(flatten)]
    pub user: UserVo,
    pub roles: Vec<RoleBriefVo>,
}

#[derive(Debug, Serialize)]
pub struct RoleBriefVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub is_super: i8,
}

impl From<UserQueryRoleRecord> for RoleBriefVo {
    fn from(role: UserQueryRoleRecord) -> Self {
        Self {
            id: role.id.to_string(),
            name: role.name,
            code: role.code,
            is_super: role.is_super,
        }
    }
}

pub struct UserService {
    authorization_resolver: AuthorizationResolver,
    authorization_cache: AuthorizationCache,
    queries: Arc<dyn UserQueryReadPort>,
    writes: Arc<dyn UserWritePersistencePort>,
    password_resets: Arc<dyn PasswordResetPersistencePort>,
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
    pub page: ValidatedPageQuery,
    pub username: Option<String>,
    pub phone: Option<String>,
    pub status: Option<String>,
    pub dept_id: Option<i64>,
}

impl UserListParams {
    pub fn page_only(page: ValidatedPageQuery) -> Self {
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
    pub fn new(
        authorization_cache: AuthorizationCache,
        identity_read: Arc<dyn IdentityAuthorizationReadPort>,
        queries: Arc<dyn UserQueryReadPort>,
        writes: Arc<dyn UserWritePersistencePort>,
        password_resets: Arc<dyn PasswordResetPersistencePort>,
    ) -> Self {
        Self {
            authorization_resolver: AuthorizationResolver::new(identity_read),
            authorization_cache,
            queries,
            writes,
            password_resets,
        }
    }
}
