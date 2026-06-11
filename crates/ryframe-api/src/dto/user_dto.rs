use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct CreateUserDto {
    #[validate(length(min = 1, max = 50, message = "用户名长度1-50"))]
    pub username: String,
    #[validate(length(min = 6, max = 100, message = "密码长度6-100"))]
    pub password: String,
    #[validate(length(min = 1, message = "昵称不能为空"))]
    pub nickname: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// 部门ID（接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失）
    pub dept_id: Option<String>,
    /// 角色ID列表（接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失）
    pub role_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct UpdateUserDto {
    #[validate(length(min = 1, message = "昵称不能为空"))]
    pub nickname: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// 部门ID（接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失）
    pub dept_id: Option<String>,
    pub status: String,
    /// 角色ID列表（接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失）
    pub role_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct ResetPasswordDto {
    #[validate(length(min = 6, max = 100, message = "密码长度6-100"))]
    pub password: String,
}

/// 修改用户状态请求
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangeStatusDto {
    /// 用户ID（接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失）
    pub user_id: String,
    pub status: String,
}
