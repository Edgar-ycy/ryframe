use serde::Deserialize;
use std::sync::LazyLock;
use validator::Validate;
use utoipa::ToSchema;

/// 更新个人信息请求
#[derive(Debug, Deserialize, Validate, ToSchema)]
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
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

impl ChangePasswordRequest {
    /// 校验密码复杂度：长度6-100，且包含字母和数字
    pub fn validate_passwords(&self) -> Result<(), String> {
        if self.old_password.len() < 6 || self.old_password.len() > 100 {
            return Err("旧密码长度为6-100个字符".into());
        }
        if self.new_password.len() < 6 || self.new_password.len() > 100 {
            return Err("新密码长度为6-100个字符".into());
        }
        let has_letter = self.new_password.chars().any(|c| c.is_alphabetic());
        let has_digit = self.new_password.chars().any(|c| c.is_ascii_digit());
        if !has_letter || !has_digit {
            return Err("密码必须包含字母和数字".into());
        }
        Ok(())
    }
}
