use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::next_id;
use async_trait::async_trait;
use futures_util::future::try_join_all;
use ryframe_auth::password;
use ryframe_kernel::{
    ActorContext, AppError, AppResult, DataScope, PageResult, ValidatedPageQuery,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime, UtcOffset};
use tokio::sync::Semaphore;
use validator::Validate;

use super::{
    file::{
        DownloadedFile, FileService, IMPORT_BUCKET, UploadCommand, UploadPolicy, UploadResponse,
    },
    user::UserService,
};
use crate::{
    ClaimedBackgroundJob, EnqueueJob, JobHandler, JobQueue,
    ports::spreadsheet::SpreadsheetDocumentProcessor,
    ports::users::{
        NewImportedUser, NewUserImportJob, NewUserImportRow, UserImportDepartmentRecord,
        UserImportJobRecord, UserImportPersistencePort, UserImportReadFilter, UserImportRowRecord,
        UserImportSourceRecord, UserImportSourceState,
    },
};

/// 可恢复用户导入的稳定后台任务类型。
pub const USER_IMPORT_JOB_TYPE: &str = "system.user.import";
const USER_IMPORT_PERMISSION: &str = "system:user-import:add";
const USER_IMPORT_MAX_RUNTIME_SECONDS: i32 = 14_400;
const USER_IMPORT_MAX_ATTEMPTS: i32 = 3;
const DEPARTMENT_PATH_SEPARATOR: &str = " / ";
const DEPARTMENT_PATH_MAX_BYTES: usize = 2_048;
const DEPARTMENT_HIERARCHY_MAX_DEPTH: usize = 128;
const IMPORT_ORPHAN_CLEANUP_GRACE_MINUTES: i64 = 5;
const USER_IMPORT_REPORT_HEADERS: &[(&str, &str)] = &[
    ("row_number", "行号"),
    ("username", "用户名"),
    ("outcome", "结果"),
    ("code", "代码"),
    ("message", "说明"),
];

#[derive(Clone)]
pub struct UserImportService {
    queue: Arc<JobQueue>,
    user: Arc<UserService>,
    file: Arc<FileService>,
    config: crate::UserImportPolicy,
    hash_permits: Arc<Semaphore>,
    spreadsheets: Arc<dyn SpreadsheetDocumentProcessor>,
    persistence: Arc<dyn UserImportPersistencePort>,
    authorization_cache: crate::AuthorizationCache,
}

include!("models.rs");
include!("management.rs");
include!("workflow.rs");
include!("lifecycle.rs");
include!("report.rs");
include!("handler.rs");
include!("department.rs");
include!("batch.rs");

fn orphan_cleanup_deadline(now: OffsetDateTime) -> AppResult<OffsetDateTime> {
    now.checked_add(Duration::minutes(IMPORT_ORPHAN_CLEANUP_GRACE_MINUTES))
        .ok_or_else(|| AppError::Database("文件时间无法精确表示为 UTC".into()))
}

#[cfg(test)]
mod import_row_tests {
    use super::*;

    #[test]
    fn exception_rows_reject_unrepresentable_batch_time_only_when_present() {
        for invalid in [
            time::Date::MIN
                .midnight()
                .assume_offset(time::macros::offset!(+01:00)),
            time::Date::MAX
                .with_hms(23, 59, 59)
                .unwrap()
                .assume_offset(time::macros::offset!(-01:00)),
        ] {
            assert!(
                build_issue_records(Vec::new(), "tenant-a", 7, invalid)
                    .unwrap()
                    .is_empty()
            );
            let issue = RowIssue::skipped(3, "tester", "already_exists", "用户已存在");
            let error = build_issue_records(vec![issue], "tenant-a", 7, invalid).unwrap_err();
            assert!(matches!(error, AppError::Database(message)
                if message.starts_with("用户导入异常行创建时间无法精确表示为 UTC")));
        }
    }
}
