use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use time::OffsetDateTime;

mod cleanup;
mod download;
mod upload;

pub use cleanup::port as cleanup;
pub use download::port as download;
pub use upload::port as upload;
pub use upload::{map_model as map_upload_model, map_record as map_upload_record};

pub(super) fn utc_time_from_chrono(instant: DateTime<Utc>) -> AppResult<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(instant.timestamp())
        .and_then(|utc| utc.replace_nanosecond(instant.timestamp_subsec_nanos()))
        .map_err(|_| AppError::Database("文件时间无法精确表示为 UTC".into()))
}
