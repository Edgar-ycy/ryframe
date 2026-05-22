use serde::Deserialize;

#[derive(Debug, Deserialize, validator::Validate)]
pub struct CreateUserDto {
    #[validate(length(min = 1, max = 50, message = "用户名长度1-50"))]
    pub username: String,
    #[validate(length(min = 6, max = 100, message = "密码长度6-100"))]
    pub password: String,
    #[validate(length(min = 1, message = "昵称不能为空"))]
    pub nickname: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub dept_id: Option<i64>,
    pub role_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize, validator::Validate)]
pub struct UpdateUserDto {
    #[validate(length(min = 1, message = "昵称不能为空"))]
    pub nickname: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub dept_id: Option<i64>,
    pub status: String,
    pub role_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize, validator::Validate)]
pub struct ResetPasswordDto {
    #[validate(length(min = 6, max = 100, message = "密码长度6-100"))]
    pub password: String,
}
