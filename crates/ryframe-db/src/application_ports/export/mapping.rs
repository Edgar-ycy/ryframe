use ryframe_application::ports::export::ExportRequesterRecord;

pub(super) fn requester_record(
    record: crate::entities::export_job::Model,
) -> ExportRequesterRecord {
    ExportRequesterRecord {
        id: record.id,
        resource: record.resource,
        status: record.status,
        result_file_name: record.result_file_name,
        content_type: record.content_type,
        file_size: record.file_size,
        expires_at: record.expires_at,
        error_message: record.error_message,
        snapshot_at: record.snapshot_at,
        matched_rows: record.matched_rows,
        created_at: record.created_at,
        updated_at: record.updated_at,
        completed_at: record.completed_at,
        notification_read_at: record.notification_read_at,
        permission_code: record.permission_code,
        request_params: record.request_params,
        request_version: record.request_version,
        authorization_fingerprint: record.authorization_fingerprint,
        upper_id: record.upper_id,
        result_file_id: record.result_file_id,
    }
}
