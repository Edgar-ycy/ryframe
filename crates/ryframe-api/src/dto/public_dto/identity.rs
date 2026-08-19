use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use ryframe_application::UserInfo as ServiceUserInfo;
use ryframe_application::system::profile_service::UserProfileResponse as ServiceUserProfileResponse;
use ryframe_application::system::{
    RoleBriefVo as ServiceRoleBriefVo, UserDetailVo as ServiceUserDetailVo, UserVo as ServiceUserVo,
};

/// 当前登录用户的公开信息。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UserInfo {
    pub id: String,
    pub tenant_id: String,
    pub tenant_name: String,
    pub dept_name: Option<String>,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub preferred_locale: Option<String>,
    pub roles: Vec<String>,
    pub perms: Vec<String>,
}

impl From<ServiceUserInfo> for UserInfo {
    fn from(value: ServiceUserInfo) -> Self {
        let ServiceUserInfo {
            id,
            tenant_id,
            tenant_name,
            dept_name,
            username,
            nickname,
            email,
            phone,
            avatar,
            preferred_locale,
            roles,
            perms,
        } = value;
        Self {
            id,
            tenant_id,
            tenant_name,
            dept_name,
            username,
            nickname,
            email,
            phone,
            avatar,
            preferred_locale,
            roles,
            perms,
        }
    }
}

/// 用户个人信息响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct UserProfileResponse {
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

impl From<ServiceUserProfileResponse> for UserProfileResponse {
    fn from(value: ServiceUserProfileResponse) -> Self {
        let ServiceUserProfileResponse {
            user_id,
            username,
            nickname,
            email,
            phone,
            avatar,
            preferred_locale,
            dept_id,
            dept_name,
            status,
            remark,
            login_ip,
            login_date,
            created_at,
            roles,
            permissions,
        } = value;
        Self {
            user_id,
            username,
            nickname,
            email,
            phone,
            avatar,
            preferred_locale,
            dept_id,
            dept_name,
            status,
            remark,
            login_ip,
            login_date,
            created_at,
            roles,
            permissions,
        }
    }
}

/// 用户响应。
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
    pub created_at: DateTime<Utc>,
}

impl From<ServiceUserVo> for UserVo {
    fn from(value: ServiceUserVo) -> Self {
        let ServiceUserVo {
            id,
            username,
            nickname,
            email,
            phone,
            avatar,
            status,
            dept_id,
            dept_name,
            remark,
            created_at,
        } = value;
        Self {
            id,
            username,
            nickname,
            email,
            phone,
            avatar,
            status,
            dept_id,
            dept_name,
            remark,
            created_at,
        }
    }
}

/// 用户关联的简要角色信息。
#[derive(Debug, Serialize, ToSchema)]
pub struct RoleBriefVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub is_super: i8,
}

impl From<ServiceRoleBriefVo> for RoleBriefVo {
    fn from(value: ServiceRoleBriefVo) -> Self {
        let ServiceRoleBriefVo {
            id,
            name,
            code,
            is_super,
        } = value;
        Self {
            id,
            name,
            code,
            is_super,
        }
    }
}

/// 用户详情响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct UserDetailVo {
    #[serde(flatten)]
    pub user: UserVo,
    pub roles: Vec<RoleBriefVo>,
}

impl From<ServiceUserDetailVo> for UserDetailVo {
    fn from(value: ServiceUserDetailVo) -> Self {
        let ServiceUserDetailVo { user, roles } = value;
        Self {
            user: user.into(),
            roles: roles.into_iter().map(RoleBriefVo::from).collect(),
        }
    }
}
