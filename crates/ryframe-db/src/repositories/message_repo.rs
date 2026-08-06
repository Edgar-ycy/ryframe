use ryframe_kernel::AppError;

mod inbox;
mod publish;
mod types;

pub use types::*;

/// 消息中心仓储。
pub struct MessageRepository;

pub(super) fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
