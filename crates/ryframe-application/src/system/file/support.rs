use ryframe_kernel::{AppError, AppResult};
use time::OffsetDateTime;

use crate::ports::files::{ArtifactStoreError, ArtifactStoreErrorKind};

pub(super) fn validate_extension(filename: &str, allowed: &[String]) -> AppResult<()> {
    let extension = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    if allowed.is_empty() || allowed.contains(&extension) {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "不支持的文件类型: .{extension}"
        )))
    }
}

pub(super) fn generate_storage_filename(original_name: &str) -> String {
    let extension = original_name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let id = uuid::Uuid::new_v4();
    if extension.is_empty() {
        id.to_string()
    } else {
        format!("{id}.{extension}")
    }
}

pub(super) fn upload_date_prefix(now: OffsetDateTime) -> String {
    let utc = now.to_utc();
    format!(
        "{:04}/{:02}/{:02}",
        utc.year(),
        u8::from(utc.month()),
        utc.day()
    )
}

pub fn map_storage_write_error(error: ArtifactStoreError) -> AppError {
    match error.kind() {
        ArtifactStoreErrorKind::InvalidLocation => {
            AppError::Validation("非法的对象存储路径".into())
        }
        ArtifactStoreErrorKind::Misconfigured => AppError::Internal("对象存储配置错误".into()),
        ArtifactStoreErrorKind::Rejected | ArtifactStoreErrorKind::NotFound => {
            AppError::Internal("对象存储拒绝写入请求".into())
        }
        ArtifactStoreErrorKind::Unavailable => {
            AppError::ServiceUnavailable("对象存储暂不可用".into())
        }
    }
}

pub fn map_storage_read_error(error: ArtifactStoreError) -> AppError {
    match error.kind() {
        ArtifactStoreErrorKind::NotFound => AppError::NotFound("文件不存在".into()),
        ArtifactStoreErrorKind::InvalidLocation => {
            AppError::Validation("非法的对象存储路径".into())
        }
        ArtifactStoreErrorKind::Misconfigured => AppError::Internal("对象存储配置错误".into()),
        ArtifactStoreErrorKind::Rejected => AppError::Internal("对象存储拒绝读取请求".into()),
        ArtifactStoreErrorKind::Unavailable => {
            AppError::ServiceUnavailable("对象存储暂不可用".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::upload_date_prefix;
    use time::macros::datetime;

    #[test]
    fn upload_date_prefix_uses_the_utc_calendar_day() {
        assert_eq!(
            upload_date_prefix(datetime!(2026-01-01 07:59:59.999999 +08:00)),
            "2025/12/31"
        );
        assert_eq!(
            upload_date_prefix(datetime!(2026-01-01 08:00:00 +08:00)),
            "2026/01/01"
        );
    }
}
