use std::sync::Arc;

use chrono::Duration;
use ryframe_config::JobConfig;
use ryframe_core::repository::Repository;
use ryframe_db::{BackgroundJobRepository, DatabaseCluster, ExportJobRepository, FileRepository};
use ryframe_kernel::AppError;

use crate::JobQueue;

use super::{
    ConfigService, DictService, LoginInfoService, OperLogService, PostService, RoleService,
    UserService,
};

mod cleanup;
mod filters;
mod lifecycle;
mod resources;
mod storage;
mod types;

pub use filters::UserExportFilters;
pub use types::{
    EXPORT_BUCKET, EXPORT_CLEANUP_JOB_TYPE, EXPORT_JOB_TYPE, ExportDownloadLocation, ExportJobVo,
    RequestExportCommand,
};

use filters::{
    CONFIG_HEADERS, ConfigExportFilters, DICT_TYPE_HEADERS, DictTypeExportFilters,
    LOGIN_LOG_HEADERS, LogExportFilters, OPER_LOG_HEADERS, POST_HEADERS, PostExportFilters,
    ROLE_HEADERS, RoleExportFilters, UserExportRow, decode_export_filters,
    deterministic_export_file_id, ensure_download_authorization_matches, export_file_location,
    should_delete_uncommitted_object, validate_job_id, validate_request_command,
};
use types::StoredExportRequest;

/// 单次清理查询的最大任务数，游标会继续排空同一时间快照下的剩余任务。
const EXPORT_CLEANUP_BATCH_SIZE: u64 = 100;

/// 异步导出任务服务。
pub struct ExportService {
    db: DatabaseCluster,
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
    storage: Arc<dyn ryframe_storage::ObjectStorage>,
    default_max_attempts: i32,
    export_max_rows: usize,
    export_retention: Duration,
    job_queue: Option<Arc<JobQueue>>,
}

impl ExportService {
    pub fn new(
        db: DatabaseCluster,
        users: Arc<UserService>,
        storage: Arc<dyn ryframe_storage::ObjectStorage>,
        jobs: &JobConfig,
    ) -> Self {
        Self {
            db: db.clone(),
            background_jobs: BackgroundJobRepository,
            exports: ExportJobRepository,
            files: FileRepository,
            roles: RoleService::new(db.clone(), crate::AuthorizationCache::disabled()),
            posts: PostService::new(db.clone()),
            configs: ConfigService::new(db.clone(), crate::AuthorizationCache::disabled()),
            dicts: DictService::new(db.clone(), None),
            oper_logs: OperLogService::new(db.clone()),
            login_infos: LoginInfoService::new(db.clone()),
            users,
            storage,
            default_max_attempts: jobs.default_max_attempts,
            export_max_rows: jobs.export_max_rows,
            export_retention: Duration::hours(i64::from(jobs.export_retention_hours)),
            job_queue: None,
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

fn storage_error(error: ryframe_storage::StorageError) -> AppError {
    AppError::ServiceUnavailable(format!("导出对象存储操作失败: {error}"))
}
