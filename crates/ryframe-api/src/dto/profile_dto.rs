use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use validator::Validate;

/// 用户个人信息响应
#[derive(Debug, Clone, Serialize)]
pub struct UserProfileResponse {
    pub user_id: i64,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub dept_id: Option<i64>,
    pub dept_name: Option<String>,
    pub status: String,
    pub remark: Option<String>,
    pub login_ip: Option<String>,
    pub login_date: Option<String>,
    pub created_at: String,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
}

/// 更新个人信息请求
#[derive(Debug, Deserialize, Validate)]
pub struct UpdateProfileRequest {
    #[validate(length(min = 1, max = 64, message = "昵称长度为1-64个字符"))]
    pub nickname: String,
    #[validate(email(message = "邮箱格式不正确"))]
    pub email: Option<String>,
    #[validate(length(max = 32, message = "手机号最多32个字符"))]
    #[validate(regex(path = *PHONE_REGEX, message = "手机号格式不正确"))]
    pub phone: Option<String>,
    pub sex: Option<String>,
}

static PHONE_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^1[3-9]\d{9}$").unwrap()
});

/// 修改密码请求
#[derive(Debug, Deserialize, Validate)]
pub struct ChangePasswordRequest {
    #[validate(length(min = 6, max = 100, message = "密码长度为6-100个字符"))]
    pub old_password: String,
    #[validate(length(min = 6, max = 100, message = "新密码长度为6-100个字符"))]
    pub new_password: String,
}
