use std::sync::Arc;

use chrono::Duration;
use ryframe_kernel::AppError;

use crate::{
    ArtifactStore, ArtifactStoreError, ExportArtifactPersistencePort, ExportCleanupPersistencePort,
    ExportDeletionPersistencePort, ExportExecutionPersistencePort, ExportRequestPersistencePort,
    ExportRequesterPersistencePort, JobQueue, SpreadsheetWriterFactory,
};

use super::{
    ConfigService, DictService, LoginInfoService, OperLogService, PostService, RoleService,
    UserService,
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
use purge::ExportPurgeUseCase;
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
use types::{PersistedExportSnapshot, StoredExportRequest};

/// 单次清理查询的最大任务数，游标会继续排空同一时间快照下的剩余任务。
const EXPORT_CLEANUP_BATCH_SIZE: u64 = 100;
const EXPORT_BATCH_SIZE: u64 = 1_000;
const EXPORT_BUSINESS_MAX_ROWS: usize = 500_000;
const EXPORT_MAX_RUNTIME_SECONDS: i32 = 1_800;
const EXPORT_MAX_RESULT_BYTES: u64 = 512 * 1024 * 1024;
const EXPORT_MAX_RUNNING_PER_TENANT: u64 = 2;
const EXPORT_STATUS_RUNNING: &str = "running";
const EXPORT_STATUS_SUCCEEDED: &str = "succeeded";
const EXPORT_STATUS_FAILED: &str = "failed";
const EXPORT_STATUS_CANCELLED: &str = "cancelled";

/// 异步导出任务服务。
pub struct ExportService {
    artifact_persistence: Arc<dyn ExportArtifactPersistencePort>,
    cleanup_persistence: Arc<dyn ExportCleanupPersistencePort>,
    deletion_persistence: Arc<dyn ExportDeletionPersistencePort>,
    execution_persistence: Arc<dyn ExportExecutionPersistencePort>,
    request_persistence: Arc<dyn ExportRequestPersistencePort>,
    requester_persistence: Arc<dyn ExportRequesterPersistencePort>,
    users: Arc<UserService>,
    roles: Arc<RoleService>,
    posts: Arc<PostService>,
    configs: Arc<ConfigService>,
    dicts: Arc<DictService>,
    oper_logs: Arc<OperLogService>,
    login_infos: Arc<LoginInfoService>,
    storage: Arc<dyn ArtifactStore>,
    spreadsheets: Arc<dyn SpreadsheetWriterFactory>,
    default_max_attempts: i32,
    export_max_rows: usize,
    export_retention: Duration,
    job_queue: Option<Arc<JobQueue>>,
    purge: ExportPurgeUseCase,
}

/// 导出用例依赖的控制库端口集合，组合根只负责装配具体实现。
pub struct ExportPersistencePorts {
    artifact: Arc<dyn ExportArtifactPersistencePort>,
    cleanup: Arc<dyn ExportCleanupPersistencePort>,
    deletion: Arc<dyn ExportDeletionPersistencePort>,
    execution: Arc<dyn ExportExecutionPersistencePort>,
    request: Arc<dyn ExportRequestPersistencePort>,
    requester: Arc<dyn ExportRequesterPersistencePort>,
}

/// 导出七类资源所复用的应用服务，由组合根统一装配。
pub struct ExportResourceServices {
    pub users: Arc<UserService>,
    pub roles: Arc<RoleService>,
    pub posts: Arc<PostService>,
    pub configs: Arc<ConfigService>,
    pub dicts: Arc<DictService>,
    pub oper_logs: Arc<OperLogService>,
    pub login_infos: Arc<LoginInfoService>,
}

impl ExportPersistencePorts {
    pub fn new(
        artifact: Arc<dyn ExportArtifactPersistencePort>,
        cleanup: Arc<dyn ExportCleanupPersistencePort>,
        deletion: Arc<dyn ExportDeletionPersistencePort>,
        execution: Arc<dyn ExportExecutionPersistencePort>,
        request: Arc<dyn ExportRequestPersistencePort>,
        requester: Arc<dyn ExportRequesterPersistencePort>,
    ) -> Self {
        Self {
            artifact,
            cleanup,
            deletion,
            execution,
            request,
            requester,
        }
    }
}

impl ExportService {
    pub fn new(
        persistence: ExportPersistencePorts,
        resources: ExportResourceServices,
        storage: Arc<dyn ArtifactStore>,
        spreadsheets: Arc<dyn SpreadsheetWriterFactory>,
        policy: crate::ExportPolicy,
    ) -> Self {
        let purge = ExportPurgeUseCase::new(Arc::clone(&persistence.cleanup), Arc::clone(&storage));
        Self {
            artifact_persistence: persistence.artifact,
            cleanup_persistence: persistence.cleanup,
            deletion_persistence: persistence.deletion,
            execution_persistence: persistence.execution,
            request_persistence: persistence.request,
            requester_persistence: persistence.requester,
            roles: resources.roles,
            posts: resources.posts,
            configs: resources.configs,
            dicts: resources.dicts,
            oper_logs: resources.oper_logs,
            login_infos: resources.login_infos,
            users: resources.users,
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

fn storage_error(error: ArtifactStoreError) -> AppError {
    AppError::ServiceUnavailable(format!("导出对象存储操作失败: {error}"))
}
