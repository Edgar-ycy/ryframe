use ryframe_application::ports::users::{
    NewImportedUser, NewUserImportRow, UserImportJobRecord, UserImportRowRecord,
    UserImportSourceRecord, UserImportSourceState,
};

use crate::entities::{sys_file, user, user_import_job, user_import_row_result};

pub(super) fn job_record(job: user_import_job::Model) -> UserImportJobRecord {
    UserImportJobRecord {
        id: job.id,
        tenant_id: job.tenant_id,
        requester_user_id: job.requester_user_id,
        background_job_id: job.background_job_id,
        idempotency_key_hash: job.idempotency_key_hash,
        source_file_id: job.source_file_id,
        source_name_snapshot: job.source_name_snapshot,
        source_sha256: job.source_sha256,
        duplicate_policy: job.duplicate_policy,
        status: job.status,
        total_rows: job.total_rows,
        processed_rows: job.processed_rows,
        success_count: job.success_count,
        skipped_count: job.skipped_count,
        failure_count: job.failure_count,
        cancel_requested: job.cancel_requested,
        error_report_file_id: job.error_report_file_id,
        last_error: job.last_error,
        started_at: job.started_at,
        completed_at: job.completed_at,
        created_at: job.created_at,
        updated_at: job.updated_at,
    }
}

pub(super) fn job_model(job: UserImportJobRecord) -> user_import_job::Model {
    user_import_job::Model {
        id: job.id,
        tenant_id: job.tenant_id,
        requester_user_id: job.requester_user_id,
        background_job_id: job.background_job_id,
        idempotency_key_hash: job.idempotency_key_hash,
        source_file_id: job.source_file_id,
        source_name_snapshot: job.source_name_snapshot,
        source_sha256: job.source_sha256,
        duplicate_policy: job.duplicate_policy,
        status: job.status,
        total_rows: job.total_rows,
        processed_rows: job.processed_rows,
        success_count: job.success_count,
        skipped_count: job.skipped_count,
        failure_count: job.failure_count,
        cancel_requested: job.cancel_requested,
        error_report_file_id: job.error_report_file_id,
        last_error: job.last_error,
        started_at: job.started_at,
        completed_at: job.completed_at,
        created_at: job.created_at,
        updated_at: job.updated_at,
    }
}

pub(super) fn row_record(row: user_import_row_result::Model) -> UserImportRowRecord {
    UserImportRowRecord {
        row_number: row.row_number,
        username: row.username_snapshot,
        outcome: row.outcome,
        code: row.code,
        message: row.message,
        created_at: row.created_at,
    }
}

pub(super) fn source_record(file: sys_file::Model) -> UserImportSourceRecord {
    let state = if file.del_flag != sys_file::Model::DEL_FLAG_NORMAL {
        UserImportSourceState::Unavailable
    } else if file.upload_status == sys_file::Model::UPLOAD_STATUS_READY {
        UserImportSourceState::Ready
    } else if file.upload_status == sys_file::Model::UPLOAD_STATUS_CLEANUP {
        UserImportSourceState::Recoverable
    } else {
        UserImportSourceState::Unavailable
    };
    UserImportSourceRecord {
        bucket: file.bucket,
        sha256: file.file_sha256,
        state,
    }
}

pub(super) fn user_model(user: NewImportedUser) -> user::Model {
    user::Model {
        id: user.id,
        tenant_id: user.tenant_id,
        username: user.username,
        password_hash: user.password_hash,
        nickname: user.nickname,
        email: user.email,
        phone: user.phone,
        avatar: None,
        avatar_file_id: None,
        preferred_locale: None,
        status: user::Model::STATUS_PENDING_ACTIVATION.to_owned(),
        authorization_version: 1,
        dept_id: Some(user.department_id),
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_owned(),
        created_at: user.created_at,
        updated_at: user.created_at,
    }
}

pub(super) fn import_row_model(row: NewUserImportRow) -> user_import_row_result::Model {
    user_import_row_result::Model {
        id: row.id,
        tenant_id: row.tenant_id,
        import_job_id: row.import_job_id,
        row_number: row.row_number,
        username_snapshot: row.username,
        outcome: row.outcome,
        code: row.code,
        message: row.message,
        created_at: row.created_at,
    }
}

pub(super) fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}
