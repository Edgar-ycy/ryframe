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
    /// 接口接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失。
    pub dept_id: Option<String>,
    /// 接口接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失。
    pub role_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateUserDto {
    #[validate(length(min = 1, message = "昵称不能为空"))]
    pub nickname: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// 接口接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失。
    pub dept_id: Option<String>,
    pub status: String,
    /// 接口接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失。
    pub role_ids: Option<Vec<String>>,
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangeStatusDto {
    /// 接口接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失。
    pub user_id: String,
    pub status: String,
}
