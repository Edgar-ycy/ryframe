use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateUserDto {
    #[validate(length(min = 1, max = 50, message = "用户名长度 1-50"))]
    pub username: String,
    #[validate(length(min = 1, message = "昵称不能为空"))]
    pub nickname: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// Snowflake ID 统一使用字符串传输，避免 JavaScript 精度丢失。
    pub dept_id: Option<String>,
    #[serde(default)]
    pub role_ids: Vec<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateUserDto {
    #[validate(length(min = 1, message = "昵称不能为空"))]
    pub nickname: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// Snowflake ID 统一使用字符串传输，避免 JavaScript 精度丢失。
    pub dept_id: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceUserRolesDto {
    #[serde(default)]
    pub role_ids: Vec<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordResetRequestDto {
    #[validate(length(
        min = 1,
        max = 512,
        message = "密码重置原因不能为空且不能超过 512 字符"
    ))]
    pub reason: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PasswordResetRequestResponse {
    pub request_id: String,
    pub reset_token: String,
    pub reset_url: String,
    pub expires_at: String,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateUserStatusDto {
    #[validate(custom(function = "validate_user_status"))]
    pub status: String,
}

fn validate_user_status(value: &str) -> Result<(), validator::ValidationError> {
    if matches!(value, "0" | "1") {
        Ok(())
    } else {
        Err(validator::ValidationError::new("invalid_user_status"))
    }
}
