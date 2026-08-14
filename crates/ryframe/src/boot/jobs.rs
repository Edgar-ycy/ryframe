use std::sync::Arc;

use ryframe_config::{JobConfig, MultiTenancyConfig};
use ryframe_core::RedisClient;
use ryframe_db::ExecutionTenantScope;
use ryframe_kernel::{AppError, AppResult};
use ryframe_service::{
    CallbackScheduleMetricsObserver, ExportCleanupJobHandler, ExportJobHandler, JobQueue,
    JobWorker, MessageDispatchJobHandler, MessageRetentionJobHandler, ScheduleMetricsObserver,
    ScheduledJobTargetRegistry,
    system::{
        DataRetentionJobHandler, DataRetentionService, ExportService, MessageService,
        TenantConfigApplyJobHandler, TenantConfigExportJobHandler, TenantConfigPreviewJobHandler,
        TenantConfigRollbackJobHandler, TenantConfigTransferService, UserImportJobHandler,
        UserImportService,
    },
};

/// 构造内置后台任务处理器所需的业务服务。
pub struct JobWorkerDependencies {
    pub export: Arc<ExportService>,
    pub message: Arc<MessageService>,
    pub data_retention: Arc<DataRetentionService>,
    pub user_import: Arc<UserImportService>,
    pub tenant_config_transfer: Arc<TenantConfigTransferService>,
    pub redis: Option<RedisClient>,
    pub messaging_enabled: bool,
}

/// 统一构造 Embedded 与 External 模式使用的后台任务处理器。
pub fn build_job_worker(
    queue: Arc<JobQueue>,
    config: &JobConfig,
    execution_tenant_scope: ExecutionTenantScope,
    dependencies: JobWorkerDependencies,
) -> AppResult<JobWorker> {
    let worker = JobWorker::new(queue, config, execution_tenant_scope)?
        .with_handler(Arc::new(ExportJobHandler::new(dependencies.export.clone())))?
        .with_handler(Arc::new(ExportCleanupJobHandler::new(dependencies.export)))?
        .with_handler(Arc::new(DataRetentionJobHandler::new(
            dependencies.data_retention,
        )))?
        .with_handler(Arc::new(UserImportJobHandler::new(
            dependencies.user_import,
        )))?
        .with_handler(Arc::new(TenantConfigExportJobHandler::new(
            dependencies.tenant_config_transfer.clone(),
        )))?
        .with_handler(Arc::new(TenantConfigPreviewJobHandler::new(
            dependencies.tenant_config_transfer.clone(),
        )))?
        .with_handler(Arc::new(TenantConfigApplyJobHandler::new(
            dependencies.tenant_config_transfer.clone(),
        )))?
        .with_handler(Arc::new(TenantConfigRollbackJobHandler::new(
            dependencies.tenant_config_transfer,
        )))?;
    if !dependencies.messaging_enabled {
        return Ok(worker);
    }
    worker
        .with_handler(Arc::new(
            MessageDispatchJobHandler::new(dependencies.message.clone(), dependencies.redis)
                .with_redis_wakeup_failure_observer(Arc::new(|| {
                    ryframe_middleware::metrics::record_redis_degraded("message_dispatch_wakeup");
                })),
        ))?
        .with_handler(Arc::new(
            MessageRetentionJobHandler::new(dependencies.message).with_deleted_observer(Arc::new(
                ryframe_middleware::metrics::record_message_retention_deleted,
            )),
        ))
}

/// 将应用的多租户开关转换为后台执行器使用的数据库范围。
pub fn execution_tenant_scope(config: &MultiTenancyConfig) -> ExecutionTenantScope {
    config.fixed_tenant_id().map_or_else(
        ExecutionTenantScope::all,
        ExecutionTenantScope::tenant_and_platform,
    )
}

/// 统一构造 API 与 Worker 使用的内置调度目标目录。
pub fn build_schedule_targets(messaging_enabled: bool) -> AppResult<ScheduledJobTargetRegistry> {
    ScheduledJobTargetRegistry::built_in(messaging_enabled)
}

/// 构造只属于 Cron 功能边界的低基数指标观察者。
pub fn build_schedule_metrics_observer() -> Arc<dyn ScheduleMetricsObserver> {
    Arc::new(CallbackScheduleMetricsObserver::new(
        Arc::new(ryframe_middleware::metrics::record_job_schedule_scan),
        Arc::new(ryframe_middleware::metrics::record_job_schedule_trigger),
        Arc::new(ryframe_middleware::metrics::observe_job_schedule_lag),
    ))
}

/// 在应用启动边界校验可用调度目标和通用任务处理器的一致性。
pub fn validate_schedule_targets(
    worker: &JobWorker,
    targets: &ScheduledJobTargetRegistry,
) -> AppResult<()> {
    let missing = targets
        .available_job_types()
        .into_iter()
        .filter(|job_type| !worker.has_handler(job_type))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    Err(AppError::Config(format!(
        "调度目标缺少后台任务处理器: {}",
        missing.join(", ")
    )))
}
