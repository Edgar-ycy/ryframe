use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::future::try_join_all;
use ryframe_adapters::excel::{ExcelExporter, ExcelImportRow, ExcelImporter};
use ryframe_adapters::snowflake::try_next_snowflake_id;
use ryframe_auth::password;
use ryframe_config::UserImportConfig;
use ryframe_db::{
    ControlDatabaseCluster, CreateUserImportJob, DeptRepository, EnqueueBackgroundJob,
    FileRepository, TenantConfigTransferRepository, TenantRepository, UserImportFilter,
    UserImportRepository, UserRepository, background_job,
    entities::{dept, user, user_import_job, user_import_row_result},
};
use ryframe_kernel::{
    ActorContext, AppError, AppResult, DataScope, PageResult, ValidatedPageQuery,
};
use sea_orm::{EntityTrait, TransactionTrait};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use uuid::Uuid;
use validator::Validate;

use super::{
    DownloadedFile, FileService, IMPORT_BUCKET, UploadCommand, UploadPolicy, UploadResponse,
    UserService,
};
use crate::{JobHandler, JobQueue};

/// 可恢复用户导入的稳定后台任务类型。
pub const USER_IMPORT_JOB_TYPE: &str = "system.user.import";
const USER_IMPORT_PERMISSION: &str = "system:user-import:add";
const USER_IMPORT_MAX_RUNTIME_SECONDS: i32 = 14_400;
const USER_IMPORT_MAX_ATTEMPTS: i32 = 3;
const DEPARTMENT_PATH_SEPARATOR: &str = " / ";
const DEPARTMENT_PATH_MAX_BYTES: usize = 2_048;
const DEPARTMENT_HIERARCHY_MAX_DEPTH: usize = 128;
const IMPORT_ORPHAN_CLEANUP_GRACE_MINUTES: i64 = 5;

#[derive(Clone)]
pub struct UserImportService {
    db: ControlDatabaseCluster,
    queue: Arc<JobQueue>,
    user_service: Arc<UserService>,
    file_service: Arc<FileService>,
    config: UserImportConfig,
    hash_permits: Arc<Semaphore>,
}

include!("models.rs");
include!("management.rs");
include!("workflow.rs");
include!("lifecycle.rs");
include!("report.rs");
include!("handler.rs");
include!("department.rs");
include!("batch.rs");
