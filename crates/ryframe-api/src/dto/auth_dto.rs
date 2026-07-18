use ryframe_service::{LoginResult, UserInfo};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::password_validation::validate_password_complexity;

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

#[derive(Serialize, ToSchema)]
pub struct LoginResponse {
    pub access_token: String,
    pub expires_in: usize,
    pub user_info: UserInfo,
}

impl From<LoginResult> for LoginResponse {
    fn from(r: LoginResult) -> Self {
        Self {
            access_token: r.access_token,
            expires_in: r.expires_in,
            user_info: r.user_info,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct CsrfResponse {
    pub csrf_token: String,
    pub expires_in: usize,
}
