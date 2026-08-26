use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::entities::export_job;

use super::{CreateExportJob, ExportStartDisposition};

pub(super) fn validate_create_command(command: &CreateExportJob) -> AppResult<()> {
    if command.tenant_id.is_empty() || command.tenant_id.len() > 64 {
        return Err(AppError::Validation("导出任务租户标识无效".into()));
    }
    if command.requester_id <= 0 || command.background_job_id <= 0 {
        return Err(AppError::Validation("导出任务关联标识必须为正数".into()));
    }
    if command.upper_id <= 0 || command.matched_rows <= 0 || command.request_version <= 0 {
        return Err(AppError::Validation(
            "导出任务主键上界和匹配行数必须为正数".into(),
        ));
    }
    for (name, value, maximum) in [
        ("resource", command.resource.as_str(), 64),
        ("permission_code", command.permission_code.as_str(), 128),
    ] {
        if value.is_empty() || value.len() > maximum {
            return Err(AppError::Validation(format!(
                "导出任务 {name} 长度必须介于 1 和 {maximum} 之间"
            )));
        }
    }
    for (name, value) in [
        (
            "authorization_fingerprint",
            command.authorization_fingerprint.as_str(),
        ),
        ("request_fingerprint", command.request_fingerprint.as_str()),
    ] {
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::Validation(format!(
                "导出任务 {name} 必须是 64 位十六进制指纹"
            )));
        }
    }
    Ok(())
}

pub fn visible_for_requester_query(
    tenant_id: &str,
    requester_id: i64,
) -> sea_orm::Select<export_job::Entity> {
    export_job::Entity::find()
        .filter(export_job::Column::TenantId.eq(tenant_id))
        .filter(export_job::Column::RequesterId.eq(requester_id))
        .filter(export_job::Column::DeletePendingAt.is_null())
}

pub(super) fn truncate_error(error: &str) -> String {
    const MAX_ERROR_BYTES: usize = 4_000;
    if error.len() <= MAX_ERROR_BYTES {
        return error.to_owned();
    }
    let mut end = MAX_ERROR_BYTES;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &error[..end])
}

pub fn decide_export_start(
    status: &str,
    delete_pending: bool,
    running: u64,
    maximum_running: u64,
) -> ExportStartDisposition {
    if delete_pending {
        return ExportStartDisposition::NotRunnable;
    }
    if status == export_job::Model::STATUS_RUNNING {
        return ExportStartDisposition::AlreadyRunning;
    }
    if status != export_job::Model::STATUS_QUEUED {
        return ExportStartDisposition::NotRunnable;
    }
    if running >= maximum_running {
        ExportStartDisposition::ConcurrencyLimited
    } else {
        ExportStartDisposition::Started
    }
}
