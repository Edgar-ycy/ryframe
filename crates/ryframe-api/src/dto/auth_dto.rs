use chrono::{DateTime, Utc};
use ryframe_application::{LoginResult, UserInfo as ServiceUserInfo};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::product_dto::SessionCapabilityVo;
use super::{
    fixed_value::TenantBusinessDataState, password_validation::validate_password_complexity,
    public_dto::MenuTreeNode, tenant_validation::validate_tenant_identifier,
};

#[derive(Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    #[validate(length(min = 1, max = 64, message = "用户名不能为空且不能超过64个字符"))]
    pub username: String,
    #[validate(length(min = 1, max = 256, message = "密码不能为空且不能超过256个字符"))]
    pub password: String,
    pub captcha_id: Option<String>,
    pub captcha_code: Option<String>,
}

#[derive(Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletePasswordResetRequest {
    #[validate(custom(function = "validate_tenant_identifier"))]
    #[schema(pattern = r"^[A-Za-z0-9](?:[A-Za-z0-9_-]{0,62}[A-Za-z0-9])$")]
    pub tenant_id: Option<String>,
    pub request_id: String,
    #[validate(length(min = 1, message = "重置令牌不能为空"))]
    pub token: String,
    #[validate(custom(function = "validate_password_complexity"))]
    #[schema(
        min_length = 8,
        max_length = 72,
        pattern = r"^(?=.*[A-Z])(?=.*[a-z])(?=.*[0-9])(?=.*[^A-Za-z0-9])[!-~]{8,72}$"
    )]
    pub new_password: String,
}

#[derive(Serialize, ToSchema)]
pub struct LoginResponse {
    pub access_token: String,
    pub expires_in: usize,
    pub session_context: SessionContextVo,
}

impl LoginResponse {
    pub fn new(value: LoginResult, session_context: SessionContextVo) -> Self {
        let LoginResult {
            access_token,
            refresh_token: _,
            sid: _,
            user_id: _,
            user_info: _,
            expires_in,
            refresh_expires_at: _,
        } = value;
        Self {
            access_token,
            expires_in,
            session_context,
        }
    }
}

/// 会话身份，仅包含稳定身份与展示字段；授权集合只存在于 SessionContext 顶层。
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionUserVo {
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
}

impl From<ServiceUserInfo> for SessionUserVo {
    fn from(value: ServiceUserInfo) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            tenant_name: value.tenant_name,
            dept_name: value.dept_name,
            username: value.username,
            nickname: value.nickname,
            email: value.email,
            phone: value.phone,
            avatar: value.avatar,
            preferred_locale: value.preferred_locale,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TenantBusinessDataContextVo {
    pub state: TenantBusinessDataState,
    pub placement_generation: String,
}

/// 登录、刷新和 GET /auth/context 共用的会话启动快照。
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionContextVo {
    pub user: SessionUserVo,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
    /// 控制库授权纪元；避免 JavaScript 精度漂移，所有 epoch 均以十进制字符串输出。
    pub authorization_epoch: String,
    pub runtime_epoch: String,
    pub capabilities: Vec<SessionCapabilityVo>,
    pub business_data: TenantBusinessDataContextVo,
    pub menus: Vec<MenuTreeNode>,
}

#[derive(Serialize, ToSchema)]
pub struct CsrfResponse {
    pub csrf_token: String,
    pub expires_in: usize,
}

/// 当前用户可管理的登录设备会话。
#[derive(Debug, Serialize, ToSchema)]
pub struct AuthSessionResponse {
    /// 稳定会话标识，只用于精确撤销，不是访问令牌或刷新令牌。
    pub sid: String,
    pub current: bool,
    pub ipaddr: String,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub login_time: DateTime<Utc>,
    pub last_access_time: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// 批量撤销其他登录设备的结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct RevokeOtherSessionsResponse {
    pub revoked_count: u64,
}
