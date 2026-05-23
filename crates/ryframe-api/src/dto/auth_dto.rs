use ryframe_service::{LoginResult, UserInfo};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// 登录请求
#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct LoginRequest {
    #[validate(length(min = 1, message = "用户名不能为空"))]
    pub username: String,
    #[validate(length(min = 1, message = "密码不能为空"))]
    pub password: String,
}

/// 刷新令牌请求
#[derive(Debug, Deserialize, ToSchema)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// 登录响应（面向 API 输出）
#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user_info: UserInfo,
}

impl From<LoginResult> for LoginResponse {
    fn from(r: LoginResult) -> Self {
        Self {
            access_token: r.access_token,
            refresh_token: r.refresh_token,
            user_info: r.user_info,
        }
    }
}
