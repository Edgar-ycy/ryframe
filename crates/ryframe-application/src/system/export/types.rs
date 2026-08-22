use chrono::{DateTime, Utc};
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

/// 一批导出记录被服务端受理删除后的稳定结果。
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct ExportDeletionResult {
    pub accepted_ids: Vec<i64>,
    pub accepted_count: u64,
    pub removed_unread_count: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredExportRequest {
    pub request_version: u16,
    pub selection: ExportSelection,
    pub authorization_fingerprint: String,
    pub snapshot_at: DateTime<Utc>,
    pub upper_id: i64,
    pub matched_rows: u64,
}

pub struct PersistedExportSnapshot<'a> {
    pub request_version: i32,
    pub authorization_fingerprint: &'a str,
    pub snapshot_at: &'a DateTime<Utc>,
    pub upper_id: i64,
    pub matched_rows: i64,
}

impl StoredExportRequest {
    pub fn validate(&self, expected_resource: &str) -> ryframe_kernel::AppResult<()> {
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

    pub fn validate_persisted_snapshot(
        &self,
        export: PersistedExportSnapshot<'_>,
    ) -> ryframe_kernel::AppResult<()> {
        let matched_rows = u64::try_from(export.matched_rows)
            .map_err(|_| ryframe_kernel::AppError::Validation("导出任务匹配行数无效".into()))?;
        if i32::from(self.request_version) != export.request_version {
            return Err(ryframe_kernel::AppError::Validation(
                "导出请求快照版本与任务记录不一致".into(),
            ));
        }
        if self.authorization_fingerprint != export.authorization_fingerprint {
            return Err(ryframe_kernel::AppError::Validation(
                "导出请求授权指纹与任务记录不一致".into(),
            ));
        }
        if &self.snapshot_at != export.snapshot_at {
            return Err(ryframe_kernel::AppError::Validation(
                "导出请求快照时间与任务记录不一致".into(),
            ));
        }
        if self.upper_id != export.upper_id {
            return Err(ryframe_kernel::AppError::Validation(
                "导出请求 ID 上界与任务记录不一致".into(),
            ));
        }
        if self.matched_rows != matched_rows {
            return Err(ryframe_kernel::AppError::Validation(
                "导出请求匹配行数与任务记录不一致".into(),
            ));
        }
        Ok(())
    }
}
