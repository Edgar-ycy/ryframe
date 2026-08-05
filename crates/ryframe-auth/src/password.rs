use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, Salt, SaltString},
};
use rand::RngExt as _;
use ryframe_kernel::{AppError, AppResult};

lazy_static::lazy_static! {
    /// 用户查询未命中时使用的进程内 Argon2 哈希。保留有效哈希可使未知用户和
    /// 密码错误两种尝试执行相同的高开销验证，避免账户是否存在成为时序预言机。
    static ref DUMMY_PASSWORD_HASH: String = hash("ryframe-invalid-login-secret")
        .expect("the built-in dummy password must satisfy the Argon2 limits");
}

/// 新密码最小长度。
pub const MIN_PASSWORD_LENGTH: usize = 8;
/// Argon2 接受的新密码最大长度。
pub const MAX_PASSWORD_LENGTH: usize = 72;
/// OpenAPI/浏览器端使用的等价复杂度表达式。
pub const COMPLEXITY_PATTERN: &str =
    r"^(?=.*[A-Z])(?=.*[a-z])(?=.*[0-9])(?=.*[^A-Za-z0-9])[!-~]{8,72}$";

/// 对密码进行 argon2 哈希
///
/// # 错误
/// 密码为空或超出 argon2 长度限制时返回验证失败错误
pub fn hash(password: &str) -> AppResult<String> {
    if password.is_empty() || password.len() > MAX_PASSWORD_LENGTH {
        return Err(AppError::Validation(format!(
            "密码长度必须在 1-{} 之间",
            MAX_PASSWORD_LENGTH
        )));
    }
    let mut salt_bytes = [0_u8; Salt::RECOMMENDED_LENGTH];
    rand::rng().fill(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|error| AppError::Internal(format!("密码盐生成失败: {error}")))?;
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

/// 使用持久化哈希验证给定密码；账户不存在时改用进程内虚拟哈希。
pub fn verify_or_dummy(password: &str, hash: Option<&str>) -> AppResult<bool> {
    verify(password, hash.unwrap_or(DUMMY_PASSWORD_HASH.as_str()))
}

/// 在服务启动时初始化虚拟哈希，避免首个未知账户请求具有可区分的冷路径开销。
pub fn warm_dummy_hash() {
    let _ = DUMMY_PASSWORD_HASH.as_str();
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
/// # 错误
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
