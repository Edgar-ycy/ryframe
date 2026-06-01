use crate::error::AppError;

/// 框架统一 Result 类型
///
/// 使用方式：
/// ```
/// # use ryframe_common::{AppResult, AppError};
/// fn do_something() -> AppResult<String> {
///     let user = Some("Alice".to_string())
///         .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
///     Ok(user)
/// }
/// assert!(do_something().is_ok());
/// ```
pub type AppResult<T> = Result<T, AppError>;
