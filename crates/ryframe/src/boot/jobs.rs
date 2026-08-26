use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use ryframe_adapters::RedisClient;
use ryframe_application::{
    CallbackScheduleMetricsObserver, JobQueue, JobWakeupStream, JobWakeupTransport, JobWorker,
    JobWorkerPolicy, MultiTenancyPolicy, ScheduleMetricsObserver, ScheduledJobTargetRegistry,
    ports::jobs::ExecutionTenantScope,
    system::{
        identity::UserImportService,
        operations::{DataRetentionService, ExportService, MessageService},
        platform::{TenantConfigTransferService, TenantDataMigrationService},
    },
};
use ryframe_kernel::{AppError, AppResult};

use super::background_services::BackgroundServices;

mod handlers;
mod targets;

struct RedisJobWakeupTransport {
    client: RedisClient,
}

#[async_trait]
impl JobWakeupTransport for RedisJobWakeupTransport {
    async fn publish(&self, channel: &str, payload: &str) -> Result<(), String> {
        self.client
            .publish(channel, payload)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn subscribe(&self, channel: &str) -> Result<JobWakeupStream, String> {
        let subscription = self
            .client
            .subscribe(channel)
            .await
            .map_err(|error| error.to_string())?;
        let messages = subscription.into_on_message().map(|message| {
            message
                .get_payload::<String>()
                .map_err(|error| error.to_string())
        });
        Ok(Box::pin(messages) as JobWakeupStream)
    }
}

pub fn job_wakeup_transport(client: Option<&RedisClient>) -> Option<Arc<dyn JobWakeupTransport>> {
    client.map(|client| {
        Arc::new(RedisJobWakeupTransport {
            client: client.clone(),
        }) as Arc<dyn JobWakeupTransport>
    })
}

/// 构造内置后台任务处理器所需的业务服务。
pub struct JobWorkerDependencies {
    pub export: Arc<ExportService>,
    pub message: Arc<MessageService>,
    pub data_retention: Arc<DataRetentionService>,
    pub user_import: Arc<UserImportService>,
    pub tenant_config_transfer: Arc<TenantConfigTransferService>,
    pub tenant_data_migration: Arc<TenantDataMigrationService>,
    pub redis: Option<RedisClient>,
    pub messaging_enabled: bool,
}

impl JobWorkerDependencies {
    /// 从 API 已归组服务创建 Worker 依赖，避免进程入口重复枚举具体任务。
    pub fn from_api_services(
        services: &ryframe_api::AppServices,
        redis: Option<RedisClient>,
        messaging_enabled: bool,
    ) -> Self {
        Self {
            export: services.operations.export.clone(),
            message: services.content.message.clone(),
            data_retention: services.operations.data_retention.clone(),
            user_import: services.identity.user_import.clone(),
            tenant_config_transfer: services.platform.tenant_config_transfer.clone(),
            tenant_data_migration: services.platform.tenant_data_migration.clone(),
            redis,
            messaging_enabled,
        }
    }

    /// 从独立 Worker 的共享后台服务创建依赖。
    pub fn from_background_services(
        services: &BackgroundServices,
        redis: Option<RedisClient>,
        messaging_enabled: bool,
    ) -> Self {
        Self {
            export: services.export.clone(),
            message: services.message.clone(),
            data_retention: services.data_retention.clone(),
            user_import: services.user_import.clone(),
            tenant_config_transfer: services.tenant_config_transfer.clone(),
            tenant_data_migration: services.tenant_data_migration.clone(),
            redis,
            messaging_enabled,
        }
    }
}

/// 统一构造 Embedded 与 External 模式使用的后台任务处理器。
pub fn build_job_worker(
    queue: Arc<JobQueue>,
    policy: &JobWorkerPolicy,
    execution_tenant_scope: ExecutionTenantScope,
    dependencies: JobWorkerDependencies,
) -> AppResult<JobWorker> {
    let built_in_handlers = handlers::built_in(&dependencies);
    JobWorker::new(queue, policy, execution_tenant_scope)?.with_handlers(built_in_handlers)
}

/// 将应用的多租户开关转换为后台执行器使用的数据库范围。
pub fn execution_tenant_scope(policy: MultiTenancyPolicy) -> ExecutionTenantScope {
    policy.fixed_tenant_id().map_or_else(
        ExecutionTenantScope::all,
        ExecutionTenantScope::tenant_and_platform,
    )
}

/// 统一构造 API 与 Worker 使用的内置调度目标目录。
pub fn build_schedule_targets(messaging_enabled: bool) -> AppResult<ScheduledJobTargetRegistry> {
    ScheduledJobTargetRegistry::new().with_targets(targets::built_in(messaging_enabled))
}

/// 构造只属于 Cron 功能边界的低基数指标观察者。
pub fn build_schedule_metrics_observer() -> Arc<dyn ScheduleMetricsObserver> {
    Arc::new(CallbackScheduleMetricsObserver::new(
        Arc::new(ryframe_adapters::metrics::record_job_schedule_scan),
        Arc::new(ryframe_adapters::metrics::record_job_schedule_trigger),
        Arc::new(ryframe_adapters::metrics::observe_job_schedule_lag),
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
