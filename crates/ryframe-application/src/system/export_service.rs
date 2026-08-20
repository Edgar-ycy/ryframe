use std::sync::Arc;

use chrono::Duration;
use ryframe_db::{
    BackgroundJobRepository, ControlDatabaseCluster, ExportJobRepository, FileRepository,
    Repository,
};
use ryframe_kernel::AppError;

use crate::{ArtifactStore, ArtifactStoreError, JobQueue, SpreadsheetWriterFactory};

use super::{
    ConfigService, DictService, LoginInfoService, OperLogService, PostService, ProductService,
    RoleService, UserService,
};

mod cleanup;
mod filters;
mod lifecycle;
mod preflight;
mod purge;
mod resources;
mod storage;
mod types;

pub use filters::{
    ConfigExportFilter, DictTypeExportFilter, ExportSelection, LoginLogExportFilter,
    OperLogExportFilter, PostExportFilter, RoleExportFilter, UserExportFilter,
};
pub use purge::ExportPurgeUseCase;
pub use types::{
    EXPORT_BUCKET, EXPORT_CLEANUP_JOB_TYPE, EXPORT_JOB_TYPE, EXPORT_REQUEST_VERSION,
    ExportDeletionResult, ExportDownloadLocation, ExportJobPayload, ExportJobVo,
    RequestExportCommand,
};

use filters::{
    CONFIG_HEADERS, DICT_TYPE_HEADERS, LOGIN_LOG_HEADERS, OPER_LOG_HEADERS, POST_HEADERS,
    ROLE_HEADERS, USER_HEADERS, deterministic_export_file_id,
    ensure_download_authorization_matches, export_file_location, should_delete_uncommitted_object,
    validate_job_id, validate_request_command,
};
use types::StoredExportRequest;

/// 单次清理查询的最大任务数，游标会继续排空同一时间快照下的剩余任务。
const EXPORT_CLEANUP_BATCH_SIZE: u64 = 100;
const EXPORT_BATCH_SIZE: u64 = 1_000;
const EXPORT_BUSINESS_MAX_ROWS: usize = 500_000;
const EXPORT_MAX_RUNTIME_SECONDS: i32 = 1_800;
const EXPORT_MAX_RESULT_BYTES: u64 = 512 * 1024 * 1024;
const EXPORT_MAX_RUNNING_PER_TENANT: u64 = 2;

/// 异步导出任务服务。
pub struct ExportService {
    db: ControlDatabaseCluster,
    background_jobs: BackgroundJobRepository,
    exports: ExportJobRepository,
    files: FileRepository,
    users: Arc<UserService>,
    roles: RoleService,
    posts: PostService,
    configs: ConfigService,
    dicts: DictService,
    oper_logs: OperLogService,
    login_infos: LoginInfoService,
    storage: Arc<dyn ArtifactStore>,
    spreadsheets: Arc<dyn SpreadsheetWriterFactory>,
    default_max_attempts: i32,
    export_max_rows: usize,
    export_retention: Duration,
    job_queue: Option<Arc<JobQueue>>,
    purge: ExportPurgeUseCase,
}

impl ExportService {
    pub fn new(
        db: ControlDatabaseCluster,
        users: Arc<UserService>,
        storage: Arc<dyn ArtifactStore>,
        spreadsheets: Arc<dyn SpreadsheetWriterFactory>,
        policy: crate::ExportPolicy,
    ) -> Self {
        let purge = ExportPurgeUseCase::new(db.clone(), Arc::clone(&storage));
        Self {
            db: db.clone(),
            background_jobs: BackgroundJobRepository,
            exports: ExportJobRepository,
            files: FileRepository,
            roles: RoleService::new(
                db.clone(),
                crate::AuthorizationCache::disabled(),
                Arc::new(ProductService::new(
                    db.clone(),
                    crate::AuthorizationCache::disabled(),
                    false,
                )),
            ),
            posts: PostService::new(crate::legacy_post_persistence(db.clone())),
            configs: ConfigService::new(db.clone(), crate::AuthorizationCache::disabled()),
            dicts: DictService::new(db.clone(), None),
            oper_logs: OperLogService::new(db.clone()),
            login_infos: LoginInfoService::new(db),
            users,
            storage,
            spreadsheets,
            default_max_attempts: policy.default_max_attempts,
            export_max_rows: policy.max_rows.min(EXPORT_BUSINESS_MAX_ROWS),
            export_retention: policy.retention,
            job_queue: None,
            purge,
        }
    }

    /// 连接共享任务队列，以便在导出任务事务提交后发送可选唤醒提示。
    pub fn with_job_queue(mut self, job_queue: Arc<JobQueue>) -> Self {
        self.job_queue = Some(job_queue);
        self
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}

fn storage_error(error: ArtifactStoreError) -> AppError {
    AppError::ServiceUnavailable(format!("导出对象存储操作失败: {error}"))
}
