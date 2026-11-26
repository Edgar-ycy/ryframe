mod attempts;
mod claim;
mod enqueue;
mod stats;
mod transitions;
mod types;

pub use types::{
    BackgroundJobFilter, BackgroundJobRepository, BackgroundJobStats, BackgroundJobTypeStats,
    DeferBackgroundJob, EnqueueBackgroundJob, EnqueueBackgroundJobResult, ExpiredLeaseRecovery,
    FailBackgroundJob, JobFailureDisposition,
};

use ryframe_kernel::{AppError, AppResult};
use time::Duration;

pub(super) fn validate_lease(worker_id: &str, lease_duration: Duration) -> AppResult<()> {
    if worker_id.trim().is_empty() || worker_id.len() > 128 {
        return Err(AppError::Validation(
            "background job worker_id must contain 1 to 128 bytes".into(),
        ));
    }
    if lease_duration <= Duration::ZERO {
        return Err(AppError::Validation(
            "background job lease duration must be positive".into(),
        ));
    }
    Ok(())
}
