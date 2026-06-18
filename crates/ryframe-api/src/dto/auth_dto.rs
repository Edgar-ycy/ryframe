use ryframe_service::{LoginResult, UserInfo};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct LoginRequest {
    #[validate(length(min = 1, message = "用户名不能为空"))]
    pub username: String,
    #[validate(length(min = 1, message = "密码不能为空"))]
    pub password: String,
    pub captcha_id: Option<String>,
    pub captcha_code: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletePasswordResetRequest {
    pub request_id: String,
    #[validate(length(min = 1, message = "重置令牌不能为空"))]
    pub token: String,
    #[validate(length(min = 6, max = 72, message = "密码长度必须在 6-72 个字符之间"))]
    pub new_password: String,
}

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
