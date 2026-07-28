use crate::AppError;

/// 框架统一的领域结果类型。
pub type AppResult<T> = Result<T, AppError>;
