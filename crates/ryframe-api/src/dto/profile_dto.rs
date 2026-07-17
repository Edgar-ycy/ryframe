use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use super::password_validation::validate_password_complexity;

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateProfileRequest {
    #[validate(length(min = 1, max = 64, message = "昵称长度 1-64 个字符"))]
    pub nickname: String,
    #[validate(email(message = "邮箱格式不正确"))]
    pub email: Option<String>,
    #[validate(length(max = 32, message = "手机号最多 32 个字符"))]
    #[validate(regex(path = *PHONE_REGEX, message = "手机号格式不正确"))]
    pub phone: Option<String>,
}

static PHONE_REGEX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^1[3-9]\d{9}$").unwrap());

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangePasswordRequest {
    #[validate(length(min = 1, max = 72, message = "旧密码长度必须在 1-72 个字符之间"))]
    #[schema(min_length = 1, max_length = 72)]
    pub old_password: String,
    #[validate(custom(function = "validate_password_complexity"))]
    #[schema(
        min_length = 8,
        max_length = 72,
        pattern = r"^(?=.*[A-Z])(?=.*[a-z])(?=.*[0-9])(?=.*[^A-Za-z0-9])[!-~]{8,72}$"
    )]
    pub new_password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AvatarResponse {
    pub avatar_url: String,
}
