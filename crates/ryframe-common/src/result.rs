use crate::error::AppError;

/// 框架统一 Result 类型
///
/// 使用方式：
/// ```ignore
/// fn do_something() -> AppResult<User> {
///     let user = find_user().ok_or(AppError::NotFound("用户不存在".into()))?;
///     Ok(user)
/// }
/// ```
pub type AppResult<T> = Result<T, AppError>;
