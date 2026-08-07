mod claim;
mod enqueue;
mod stats;
mod transitions;
mod types;

pub use types::{
    BackgroundJobFilter, BackgroundJobRepository, BackgroundJobStats, BackgroundJobTypeStats,
    EnqueueBackgroundJob, EnqueueBackgroundJobResult, ExpiredLeaseRecovery, JobFailureDisposition,
};

use chrono::Duration;
use ryframe_kernel::{AppError, AppResult};

pub(super) fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

pub(super) fn validate_lease(worker_id: &str, lease_duration: Duration) -> AppResult<()> {
    if worker_id.trim().is_empty() || worker_id.len() > 128 {
        return Err(AppError::Validation(
            "background job worker_id must contain 1 to 128 bytes".into(),
        ));
    }
    if lease_duration <= Duration::zero() {
        return Err(AppError::Validation(
            "background job lease duration must be positive".into(),
        ));
    }
    Ok(())
}
