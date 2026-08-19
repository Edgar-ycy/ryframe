use chrono::{DateTime, Utc};
use ryframe_db::entities::export_job;
use serde::{Deserialize, Serialize};

use super::ExportSelection;

/// Worker 消费异步导出任务的稳定类型标识。
pub const EXPORT_JOB_TYPE: &str = "system.export.execute";

/// 清理过期导出结果的稳定任务类型标识。
pub const EXPORT_CLEANUP_JOB_TYPE: &str = "system.export.cleanup";

/// 导出文件的对象存储桶名称。
pub const EXPORT_BUCKET: &str = "exports";

/// 当前 Worker 严格接受的导出请求快照版本。
pub const EXPORT_REQUEST_VERSION: u16 = 2;

/// 后台队列中只保存可校验的导出任务定位信息。
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExportJobPayload {
    resource: String,
    request_version: u16,
}

impl ExportJobPayload {
    pub fn new(resource: &str) -> Self {
        Self {
            resource: resource.to_owned(),
            request_version: EXPORT_REQUEST_VERSION,
        }
    }

    pub fn validate(&self) -> ryframe_kernel::AppResult<()> {
        if self.request_version != EXPORT_REQUEST_VERSION {
            return Err(ryframe_kernel::AppError::Validation(format!(
                "不支持的导出后台任务版本: {}",
                self.request_version
            )));
        }
        if !matches!(
            self.resource.as_str(),
            "users" | "roles" | "posts" | "configs" | "dict-types" | "operlogs" | "loginlogs"
        ) {
            return Err(ryframe_kernel::AppError::Validation(format!(
                "不支持的导出资源: {}",
                self.resource
            )));
        }
        Ok(())
    }

    pub fn resource(&self) -> &str {
        &self.resource
    }
}

/// 创建公开导出任务的通用参数。
#[derive(Clone, Debug)]
pub struct RequestExportCommand {
    pub permission_code: String,
    pub selection: ExportSelection,
    pub confirm_all: bool,
}

/// 面向 API 的导出任务安全视图，不暴露内部后台任务载荷。
#[derive(Clone, Debug, Serialize)]
pub struct ExportJobVo {
    pub id: String,
    pub resource: String,
    pub status: String,
    pub result_file_name: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub snapshot_at: DateTime<Utc>,
    pub matched_rows: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub notification_read_at: Option<DateTime<Utc>>,
}

/// 已完成导出对应的受控文件定位信息。
#[derive(Clone, Debug)]
pub struct ExportDownloadLocation {
    pub bucket: String,
    pub path: String,
}

impl From<export_job::Model> for ExportJobVo {
    fn from(job: export_job::Model) -> Self {
        Self {
            id: job.id.to_string(),
            resource: job.resource,
            status: job.status,
            result_file_name: job.result_file_name,
            content_type: job.content_type,
            file_size: job.file_size,
            expires_at: job.expires_at,
            error_message: job.error_message,
            snapshot_at: job.snapshot_at,
            matched_rows: job.matched_rows,
            created_at: job.created_at,
            updated_at: job.updated_at,
            completed_at: job.completed_at,
            notification_read_at: job.notification_read_at,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredExportRequest {
    pub(super) request_version: u16,
    pub(super) selection: ExportSelection,
    pub(super) authorization_fingerprint: String,
    pub(super) snapshot_at: DateTime<Utc>,
    pub(super) upper_id: i64,
    pub(super) matched_rows: u64,
}

impl StoredExportRequest {
    pub(super) fn validate(&self, expected_resource: &str) -> ryframe_kernel::AppResult<()> {
        if self.request_version != EXPORT_REQUEST_VERSION {
            return Err(ryframe_kernel::AppError::Validation(format!(
                "不支持的导出请求快照版本: {}",
                self.request_version
            )));
        }
        if self.selection.resource() != expected_resource {
            return Err(ryframe_kernel::AppError::Validation(
                "导出请求快照资源与任务资源不一致".into(),
            ));
        }
        if self.authorization_fingerprint.trim().is_empty() {
            return Err(ryframe_kernel::AppError::Validation(
                "导出请求快照缺少授权指纹".into(),
            ));
        }
        if self.upper_id <= 0 || self.matched_rows == 0 {
            return Err(ryframe_kernel::AppError::Validation(
                "导出请求快照缺少有效选择边界".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn validate_persisted_snapshot(
        &self,
        export: &export_job::Model,
    ) -> ryframe_kernel::AppResult<()> {
        let matched_rows = u64::try_from(export.matched_rows)
            .map_err(|_| ryframe_kernel::AppError::Validation("导出任务匹配行数无效".into()))?;
        if self.snapshot_at != export.snapshot_at
            || self.upper_id != export.upper_id
            || self.matched_rows != matched_rows
        {
            return Err(ryframe_kernel::AppError::Validation(
                "导出请求快照与任务选择边界不一致".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::RoleExportFilter;

    #[test]
    fn worker_accepts_only_current_strict_snapshot() {
        let valid = serde_json::json!({
            "request_version": EXPORT_REQUEST_VERSION,
            "selection": {
                "resource": "roles",
                "filter": {"name": "ops", "code": null, "status": "0"}
            },
            "authorization_fingerprint": "fingerprint-at-request",
            "snapshot_at": "2026-08-20T12:00:00Z",
            "upper_id": 88,
            "matched_rows": 12
        });
        let request: StoredExportRequest =
            serde_json::from_value(valid).expect("当前版本快照应可解析");
        request.validate("roles").expect("资源应匹配");

        let previous_version = serde_json::json!({
            "request_version": EXPORT_REQUEST_VERSION - 1,
            "selection": {
                "resource": "roles",
                "filter": {"name": "ops", "code": null, "status": "0"}
            },
            "authorization_fingerprint": "fingerprint-at-request",
            "snapshot_at": "2026-08-20T12:00:00Z",
            "upper_id": 88,
            "matched_rows": 12
        });
        let previous: StoredExportRequest =
            serde_json::from_value(previous_version).expect("旧版本结构仍可被类型读取");
        assert!(matches!(
            previous.validate("roles"),
            Err(ryframe_kernel::AppError::Validation(_))
        ));

        let old_shape = serde_json::json!({"request": {"name": "ops"}});
        assert!(serde_json::from_value::<StoredExportRequest>(old_shape).is_err());

        let unknown = serde_json::json!({
            "request_version": EXPORT_REQUEST_VERSION,
            "selection": {
                "resource": "roles",
                "filter": {"name": null, "code": null, "status": null}
            },
            "authorization_fingerprint": "fingerprint-at-request",
            "snapshot_at": "2026-08-20T12:00:00Z",
            "upper_id": 88,
            "matched_rows": 12,
            "legacy": true
        });
        assert!(serde_json::from_value::<StoredExportRequest>(unknown).is_err());
    }

    #[test]
    fn worker_rejects_version_or_resource_mismatch() {
        let request = StoredExportRequest {
            request_version: EXPORT_REQUEST_VERSION + 1,
            selection: ExportSelection::Roles(RoleExportFilter::new(None, None, None)),
            authorization_fingerprint: "fingerprint-at-request".into(),
            snapshot_at: DateTime::parse_from_rfc3339("2026-08-20T12:00:00Z")
                .expect("测试时间有效")
                .with_timezone(&Utc),
            upper_id: 88,
            matched_rows: 12,
        };
        assert!(matches!(
            request.validate("roles"),
            Err(ryframe_kernel::AppError::Validation(_))
        ));

        let current = StoredExportRequest {
            request_version: EXPORT_REQUEST_VERSION,
            selection: request.selection,
            authorization_fingerprint: request.authorization_fingerprint,
            snapshot_at: request.snapshot_at,
            upper_id: request.upper_id,
            matched_rows: request.matched_rows,
        };
        assert!(matches!(
            current.validate("users"),
            Err(ryframe_kernel::AppError::Validation(_))
        ));
    }

    #[test]
    fn job_payload_rejects_old_version_and_unknown_fields() {
        let valid: ExportJobPayload = serde_json::from_value(serde_json::json!({
            "resource": "users",
            "request_version": EXPORT_REQUEST_VERSION
        }))
        .expect("当前载荷应可解析");
        valid.validate().expect("当前载荷应可校验");

        let unknown = serde_json::json!({
            "resource": "users",
            "request_version": EXPORT_REQUEST_VERSION,
            "legacy": true
        });
        assert!(serde_json::from_value::<ExportJobPayload>(unknown).is_err());

        let old = ExportJobPayload {
            resource: "users".into(),
            request_version: 0,
        };
        assert!(matches!(
            old.validate(),
            Err(ryframe_kernel::AppError::Validation(_))
        ));
    }
}
