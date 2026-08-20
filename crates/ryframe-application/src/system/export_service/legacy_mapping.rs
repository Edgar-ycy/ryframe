use ryframe_db::entities::export_job;

use super::{ExportJobVo, PersistedExportSnapshot};

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

impl<'a> From<&'a export_job::Model> for PersistedExportSnapshot<'a> {
    fn from(job: &'a export_job::Model) -> Self {
        Self {
            request_version: job.request_version,
            authorization_fingerprint: &job.authorization_fingerprint,
            snapshot_at: &job.snapshot_at,
            upper_id: job.upper_id,
            matched_rows: job.matched_rows,
        }
    }
}
