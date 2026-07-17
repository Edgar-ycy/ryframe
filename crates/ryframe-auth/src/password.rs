use argon2::{
    Argon2, PasswordHash, PasswordVerifier,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
use ryframe_common::{AppError, AppResult};

/// 新密码最小长度。
pub const MIN_PASSWORD_LENGTH: usize = 8;
/// Argon2 接受的新密码最大长度。
pub const MAX_PASSWORD_LENGTH: usize = 72;
/// OpenAPI/浏览器端使用的等价复杂度表达式。
pub const COMPLEXITY_PATTERN: &str =
    r"^(?=.*[A-Z])(?=.*[a-z])(?=.*[0-9])(?=.*[^A-Za-z0-9])[!-~]{8,72}$";

/// 对密码进行 argon2 哈希
///
/// # Errors
/// 密码为空或超出 argon2 长度限制时返回验证失败错误
pub fn hash(password: &str) -> AppResult<String> {
    if password.is_empty() || password.len() > MAX_PASSWORD_LENGTH {
        return Err(AppError::Validation(format!(
            "密码长度必须在 1-{} 之间",
            MAX_PASSWORD_LENGTH
        )));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(format!("密码哈希失败: {}", e)))
}

/// 验证密码是否匹配哈希值
pub fn verify(password: &str, hash: &str) -> AppResult<bool> {
    let parsed_hash = PasswordHash::new(hash)
        .map_err(|e| AppError::Internal(format!("密码哈希解析失败: {}", e)))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

/// 密码复杂度校验
///
/// 要求：
/// - 长度 >= 8 且 <= 72
/// - 至少包含一个大写字母
/// - 至少包含一个小写字母
/// - 至少包含一个数字
/// - 至少包含一个特殊字符
/// - 仅包含可见 ASCII 字符且不包含空格
///
/// # Errors
/// 不满足任一要求时返回 AppError::Validation
pub fn validate_complexity(password: &str) -> AppResult<()> {
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err(AppError::Validation(format!(
            "密码长度不能少于 {} 个字符",
            MIN_PASSWORD_LENGTH
        )));
    }
    if password.len() > MAX_PASSWORD_LENGTH {
        return Err(AppError::Validation(format!(
            "密码长度不能超过 {} 个字符",
            MAX_PASSWORD_LENGTH
        )));
    }
    if !password.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(AppError::Validation(
            "密码只能包含可见 ASCII 字符且不能包含空格".into(),
        ));
    }

    let has_upper = password.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = password.chars().any(|c| c.is_ascii_lowercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_special = password.chars().any(|c| !c.is_ascii_alphanumeric());

    if !has_upper {
        return Err(AppError::Validation("密码必须包含至少一个大写字母".into()));
    }
    if !has_lower {
        return Err(AppError::Validation("密码必须包含至少一个小写字母".into()));
    }
    if !has_digit {
        return Err(AppError::Validation("密码必须包含至少一个数字".into()));
    }
    if !has_special {
        return Err(AppError::Validation("密码必须包含至少一个特殊字符".into()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_complexity_too_short() {
        let err = validate_complexity("Ab1!").unwrap_err();
        assert!(err.to_string().contains("不能少于"));
    }

    #[test]
    fn test_validate_complexity_no_upper() {
        let err = validate_complexity("abcdef1!").unwrap_err();
        assert!(err.to_string().contains("大写字母"));
    }

    #[test]
    fn test_validate_complexity_no_lower() {
        let err = validate_complexity("ABCDEF1!").unwrap_err();
        assert!(err.to_string().contains("小写字母"));
    }

    #[test]
    fn test_validate_complexity_no_digit() {
        let err = validate_complexity("Abcdefg!").unwrap_err();
        assert!(err.to_string().contains("数字"));
    }

    #[test]
    fn test_validate_complexity_no_special() {
        let err = validate_complexity("Abcdefg1").unwrap_err();
        assert!(err.to_string().contains("特殊字符"));
    }

    #[test]
    fn test_validate_complexity_rejects_non_ascii_and_spaces() {
        assert!(validate_complexity("Abcdef1！").is_err());
        assert!(validate_complexity("Abc def1!").is_err());
    }

    #[test]
    fn test_validate_complexity_valid() {
        assert!(validate_complexity("Abcdef1!").is_ok());
        assert!(validate_complexity("P@ssw0rd").is_ok());
        assert!(validate_complexity("Str0ng!Pass").is_ok());
    }
}
