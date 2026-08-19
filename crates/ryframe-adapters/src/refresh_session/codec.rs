use ryframe_kernel::{AppError, AppResult};

pub(super) fn ensure_not_expired(absolute_exp: i64) -> AppResult<()> {
    if absolute_exp <= chrono::Utc::now().timestamp() {
        return Err(AppError::Authentication("refresh session expired".into()));
    }
    Ok(())
}

pub(super) fn redis_unavailable(error: redis::RedisError) -> AppError {
    tracing::error!(%error, "refresh session Redis operation failed");
    AppError::ServiceUnavailable("session service unavailable".into())
}

pub(super) fn redis_response_unavailable(message: &str) -> AppError {
    tracing::error!(message, "refresh session Redis response is invalid");
    AppError::ServiceUnavailable("session service unavailable".into())
}
