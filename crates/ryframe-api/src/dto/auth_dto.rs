use ryframe_service::{LoginResult, UserInfo};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::password_validation::validate_password_complexity;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    #[validate(length(min = 1, message = "用户名不能为空"))]
    pub username: String,
    #[validate(length(min = 1, message = "密码不能为空"))]
    pub password: String,
    pub captcha_id: Option<String>,
    pub captcha_code: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletePasswordResetRequest {
    #[validate(length(min = 1, max = 64, message = "租户ID不能为空且不能超过64个字符"))]
    pub tenant_id: String,
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
