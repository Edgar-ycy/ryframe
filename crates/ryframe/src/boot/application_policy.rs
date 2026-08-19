use std::sync::Arc;

use ryframe_application::{
    AuthPolicy, CacheAvailabilityPolicy, ExportPolicy, JobRuntimePolicy, JobSchedulePolicy,
    JobWorkerMode, JobWorkerPolicy, MessagingPolicy, MultiTenancyPolicy, PepperKeyring,
    ServiceAccountPolicy, TenantConfigTransferPolicy, UserImportPolicy,
    system::DataRetentionPolicy,
};
use ryframe_config::{AppConfig, JobWorkerMode as ConfigJobWorkerMode, RedisMode};
use ryframe_kernel::{AppError, AppResult};

/// 组合根从部署配置提取的全部应用用例运行策略。
// 该模块也由独立 Worker 复用；Worker 不装配 HTTP 专属策略。
#[allow(dead_code)]
pub struct ApplicationPolicies {
    pub auth: AuthPolicy,
    pub cache: CacheAvailabilityPolicy,
    pub job_worker: JobWorkerPolicy,
    pub job_schedule: JobSchedulePolicy,
    pub job_runtime: JobRuntimePolicy,
    pub export: ExportPolicy,
    pub retention: DataRetentionPolicy,
    pub user_import: UserImportPolicy,
    pub tenant_config_transfer: TenantConfigTransferPolicy,
    pub messaging: MessagingPolicy,
    pub service_accounts: ServiceAccountPolicy,
    pub multi_tenancy: MultiTenancyPolicy,
}

impl ApplicationPolicies {
    /// 在组合根完成配置类型到应用值对象的单向映射。
    pub fn from_config(config: &AppConfig) -> AppResult<Self> {
        let jobs = &config.jobs;
        let messaging = &config.messaging;
        let service_accounts = &config.service_accounts;
        let transfer = &config.tenant_config_transfer;
        let retention = &config.data_retention;
        let user_import = &config.user_import;
        Ok(Self {
            auth: AuthPolicy::new(
                config.auth.max_login_attempts,
                config.auth.lockout_duration_minutes,
            )?,
            cache: match config.redis.as_ref().map(|redis| redis.mode) {
                Some(RedisMode::Required) => CacheAvailabilityPolicy::Required,
                _ => CacheAvailabilityPolicy::Optional,
            },
            job_worker: JobWorkerPolicy::new(
                jobs.worker_id.as_deref(),
                jobs.lease_seconds,
                jobs.heartbeat_seconds,
                jobs.poll_interval_ms,
                jobs.max_idle_poll_interval_ms,
                jobs.lease_recovery_interval_seconds,
                jobs.concurrency,
            )?,
            job_schedule: JobSchedulePolicy::new(
                jobs.scheduler_enabled,
                jobs.scheduler_poll_interval_ms,
                jobs.scheduler_batch_size,
                jobs.max_enabled_schedules_per_tenant,
            )?,
            job_runtime: JobRuntimePolicy {
                worker_mode: map_worker_mode(jobs.mode),
                scheduler_enabled: jobs.scheduler_enabled,
            },
            export: ExportPolicy::new(
                jobs.default_max_attempts,
                jobs.export_max_rows,
                jobs.export_retention_hours,
            )?,
            retention: DataRetentionPolicy {
                cleanup_batch_size: retention.cleanup_batch_size,
                max_rows_per_resource_per_run: retention.max_rows_per_resource_per_run,
                background_job_succeeded_days: retention.background_job_succeeded_days,
                outbox_published_days: retention.outbox_published_days,
                schedule_execution_days: retention.schedule_execution_days,
                export_job_history_days: retention.export_job_history_days,
                operation_log_days: retention.operation_log_days,
                login_log_days: retention.login_log_days,
                user_import_history_days: retention.user_import_history_days,
                user_import_artifact_hours: retention.user_import_artifact_hours,
                tenant_config_artifact_hours: transfer.artifact_hours,
                tenant_config_rollback_hours: transfer.rollback_hours,
                retention_run_days: retention.retention_run_days,
                service_access_audit_days: retention.service_access_audit_days,
                dead_background_jobs_permanent: true,
                dead_outbox_events_permanent: true,
            },
            user_import: UserImportPolicy::new(
                user_import.max_file_bytes,
                user_import.max_rows,
                user_import.batch_size,
                user_import.max_active_per_tenant,
                user_import.hash_parallelism,
            )?,
            tenant_config_transfer: TenantConfigTransferPolicy::new(
                transfer.max_package_bytes,
                transfer.max_uncompressed_bytes,
                transfer.max_items,
                transfer.artifact_hours,
                transfer.rollback_hours,
                transfer.lease_seconds,
                transfer.max_runtime_seconds,
            )?,
            messaging: MessagingPolicy::new(
                messaging.enabled,
                messaging.ticket_ttl_seconds,
                messaging.retention_days,
                messaging.max_recipients_per_message,
            )?,
            service_accounts: ServiceAccountPolicy::new(
                service_accounts.enabled,
                service_accounts.max_active_credentials,
                service_accounts.max_credential_days,
                service_accounts.default_delegation_hours,
                service_accounts.max_delegation_days,
                service_accounts.default_requests_per_minute,
                service_accounts.max_concurrent_queries,
                service_accounts.query_timeout_ms,
                service_accounts.max_page_size,
                service_accounts.max_response_bytes,
            )?,
            multi_tenancy: MultiTenancyPolicy::new(config.multi_tenancy.enabled),
        })
    }
}

/// 读取部署密钥后移动所有权到应用安全类型，不复制密钥字节。
#[allow(dead_code)]
pub fn load_pepper_keyring(config: &AppConfig) -> AppResult<Arc<PepperKeyring>> {
    let loaded = config
        .service_accounts
        .load_pepper_keyring(&config.auth.jwt_secret)
        .map_err(AppError::Config)?;
    let (active_version, peppers) = loaded.into_parts();
    Ok(Arc::new(PepperKeyring::new(active_version, peppers)?))
}

const fn map_worker_mode(mode: ConfigJobWorkerMode) -> JobWorkerMode {
    match mode {
        ConfigJobWorkerMode::Embedded => JobWorkerMode::Embedded,
        ConfigJobWorkerMode::External => JobWorkerMode::External,
        ConfigJobWorkerMode::Disabled => JobWorkerMode::Disabled,
    }
}
