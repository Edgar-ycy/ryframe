//! 独立后台任务 Worker。
//!
//! 生产环境以 `ryframe-worker` 作为单独进程运行；它只验证数据库迁移状态，
//! 不执行 HTTP 服务，也不自动执行生产 DDL。

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use ryframe::boot::startup as process_startup;
use ryframe_adapters::RedisClient;
use ryframe_adapters::storage::{
    LocalObjectStorage, ObjectStorage, S3Config, S3ObjectStorage, ScopedObjectStorage,
};
use ryframe_api::monitor::DependencyHealthCache;
use ryframe_application::{
    CallbackJobMetricsObserver, JobQueue, JobScheduleService, OutboxWorker,
    ports::files::ArtifactStore,
    system::{
        CONFIG_PACKAGE_BUCKET, ConfigService, DataRetentionService, DictService, EXPORT_BUCKET,
        ExportPersistencePorts, ExportResourceServices, ExportService, FileService, IMPORT_BUCKET,
        LoginInfoService, MessageService, OperLogService, PostExportService, ProductService,
        RoleService, TenantConfigTransferService, TenantDataMigrationService, UserImportService,
        UserService,
    },
};
use ryframe_config::{
    AppConfig, Environment, JobWorkerMode, MigrationMode, RedisMode, StorageBackend,
};
use ryframe_db::{CallbackDatabaseMetricsObserver, ControlDatabaseCluster};
use ryframe_kernel::AppError;
use tokio::sync::watch;

#[path = "../boot/authorization_cache_keyspace.rs"]
mod authorization_cache_keyspace;
#[path = "../boot/application_policy.rs"]
mod process_application_policy;
#[path = "../boot/artifact_store.rs"]
mod process_artifact_store;
#[path = "../boot/authorization_cache.rs"]
mod process_authorization_cache;
#[path = "../boot/file_content.rs"]
mod process_file_content;
#[path = "../boot/jobs.rs"]
mod process_jobs;
#[path = "../boot/logging.rs"]
mod process_logging;
#[path = "../boot/readiness.rs"]
mod process_readiness;
#[path = "../boot/spreadsheet.rs"]
mod process_spreadsheet;
#[path = "../boot/tenant_config_archive.rs"]
mod process_tenant_config_archive;
#[path = "../boot/tenant_data.rs"]
mod tenant_data;

/// Worker 进程在收到关闭信号后的全部后台任务总宽限时间。
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<(), AppError> {
    ryframe_application::set_audit_failure_hook(ryframe_adapters::metrics::record_audit_failure);
    ryframe_application::set_authorization_cache_lookup_hook(
        ryframe_adapters::metrics::record_authorization_cache_lookup,
    );
    let run_mode =
        process_startup::parse_worker_run_mode(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let environment = Environment::from_env()?;
    let config = AppConfig::load_from_env(environment)?;
    if run_mode == process_startup::WorkerRunMode::Probe && config.environment.is_production() {
        return Err(AppError::Config(
            "Worker 探活模式只允许本地开发和隔离检查使用".into(),
        ));
    }
    let allows_initialization_writes = run_mode.allows_initialization_writes();
    let application_policies =
        process_application_policy::ApplicationPolicies::from_config(&config)?;
    if config.jobs.mode != JobWorkerMode::External {
        return Err(AppError::Config(
            "ryframe-worker 仅在 jobs.mode = \"external\" 时运行；embedded 由 API 进程消费，disabled 不消费任务".into(),
        ));
    }
    ryframe_adapters::snowflake::initialize(config.snowflake_worker_id)
        .map_err(|error| AppError::Config(format!("Snowflake 初始化失败: {error}")))?;
    ryframe_db::install_id_generator(|| {
        ryframe_adapters::snowflake::try_next_snowflake_id().map_err(AppError::from)
    })?;
    ryframe_application::install_id_generator(|| {
        ryframe_adapters::snowflake::try_next_snowflake_id().map_err(AppError::from)
    })?;
    let (_logger_guard, _telemetry_guard) = process_logging::init(&config)?;
    if allows_initialization_writes {
        ryframe_adapters::metrics::spawn_process_metrics_updater();
    }

    let primary = ryframe_db::connection::connect_with_sql_logging(
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await?;
    ryframe_db::connection::ping(&primary).await?;
    let database = ControlDatabaseCluster::single(primary);
    install_database_metrics(&database);

    match process_startup::effective_migration_mode(
        allows_initialization_writes,
        config.database.migration_mode,
    ) {
        MigrationMode::Auto => ryframe_db::migration::up(database.write())
            .await
            .map_err(|error| AppError::Database(format!("数据库迁移失败: {error}")))?,
        MigrationMode::Verify => ryframe_db::migration::verify(database.write())
            .await
            .map_err(|error| AppError::Database(format!("数据库迁移校验失败: {error}")))?,
        MigrationMode::Off => {
            tracing::warn!("隔离环境已关闭数据库迁移校验");
        }
    }
    ryframe_db::migration::verify_current_schema(database.write())
        .await
        .map_err(|error| AppError::Internal(format!("数据库结构指纹校验失败: {error}")))?;
    let tenant_data = Arc::new(tenant_data::build_router(database.clone(), &config)?);
    tenant_data::verify_current_targets(&tenant_data, allows_initialization_writes).await?;
    if let Some(tenant_id) = config.multi_tenancy.fixed_tenant_id() {
        ryframe_db::TenantRepository
            .ensure_available(database.write(), tenant_id)
            .await
            .map_err(|error| {
                AppError::Config(format!(
                    "单租户模式要求内置 {tenant_id} 租户存在且可用: {error}"
                ))
            })?;
        tracing::info!(tenant_id, "Worker 已启用单租户模式");
    }

    let redis = connect_redis_for_worker(&config, allows_initialization_writes).await?;
    let authorization_cache =
        process_authorization_cache::cache(redis.clone(), application_policies.cache);
    let object_storage = connect_storage_for_worker(&config, allows_initialization_writes).await?;

    let queue = Arc::new(
        JobQueue::new(ryframe_db::application_ports::jobs::queue(database.clone()))
            .with_wakeup_transport(process_jobs::job_wakeup_transport(redis.as_ref())),
    );
    let outbox_persistence = ryframe_db::application_ports::jobs::outbox(database.clone());
    install_job_metrics(&queue);
    let message = Arc::new(MessageService::new(
        ryframe_db::application_ports::system::message(database.clone()),
        queue.clone(),
        application_policies.messaging,
    ));
    let user = Arc::new(UserService::new(
        authorization_cache.clone(),
        ryframe_db::application_ports::auth::identity(database.clone()),
        ryframe_db::application_ports::users::query(database.clone()),
        ryframe_db::application_ports::users::write(database.clone(), authorization_cache.clone()),
        ryframe_db::application_ports::auth::password_reset(
            database.clone(),
            authorization_cache.clone(),
        ),
    ));
    let product = Arc::new(ProductService::new(
        ryframe_db::application_ports::product::read(database.clone()),
        ryframe_db::application_ports::product::write(database.clone()),
        authorization_cache.clone(),
        application_policies.service_accounts.enabled() && redis.is_some(),
    ));
    let role = Arc::new(RoleService::new(
        authorization_cache.clone(),
        ryframe_db::application_ports::system::role_read(database.clone()),
        ryframe_db::application_ports::system::role_write(
            database.clone(),
            authorization_cache.clone(),
            Arc::clone(&product),
        ),
    ));
    let post_export = Arc::new(PostExportService::new(
        ryframe_db::application_ports::export::post(database.clone()),
    ));
    let config_service = Arc::new(ConfigService::new(
        ryframe_db::application_ports::system::config(
            database.clone(),
            authorization_cache.clone(),
        ),
        authorization_cache.clone(),
    ));
    let dict = Arc::new(DictService::new(
        ryframe_db::application_ports::system::dict(database.clone()),
        None,
    ));
    let oper_log = Arc::new(OperLogService::new(
        ryframe_db::application_ports::system::oper_log(database.clone()),
    ));
    let login_info = Arc::new(LoginInfoService::new(
        ryframe_db::application_ports::system::login_info(database.clone()),
    ));
    let file = Arc::new(FileService::new(
        ryframe_db::application_ports::files::cleanup(database.clone()),
        ryframe_db::application_ports::files::download(database.clone()),
        ryframe_db::application_ports::files::upload(database.clone()),
        object_storage.clone(),
        process_file_content::processor(),
    ));
    if run_mode != process_startup::WorkerRunMode::Probe {
        file.spawn_upload_janitor();
    }
    let export = Arc::new(
        ExportService::new(
            ExportPersistencePorts::new(
                ryframe_db::application_ports::export::artifact(database.clone()),
                ryframe_db::application_ports::export::cleanup(database.clone()),
                ryframe_db::application_ports::export::deletion(database.clone()),
                ryframe_db::application_ports::export::execution(database.clone()),
                ryframe_db::application_ports::export::request(database.clone()),
                ryframe_db::application_ports::export::requester(database.clone()),
            ),
            ExportResourceServices {
                users: Arc::clone(&user),
                roles: role,
                posts: post_export,
                configs: config_service,
                dicts: dict,
                oper_logs: oper_log,
                login_infos: login_info,
            },
            object_storage,
            process_spreadsheet::writer_factory(),
            application_policies.export,
        )
        .with_job_queue(queue.clone()),
    );
    let data_retention = Arc::new(DataRetentionService::new(
        ryframe_db::application_ports::tenant_config::retention(database.clone()),
        ryframe_db::application_ports::retention::cleanup(database.clone()),
        ryframe_db::application_ports::retention::run(database.clone()),
        queue.clone(),
        file.clone(),
        application_policies.retention,
    ));
    let user_import = Arc::new(UserImportService::new(
        queue.clone(),
        user.clone(),
        file.clone(),
        process_spreadsheet::document_processor(),
        ryframe_db::application_ports::users::import(database.clone()),
        application_policies.user_import,
    ));
    let tenant_config_transfer = Arc::new(TenantConfigTransferService::new(
        ryframe_application::system::TenantConfigTransferDependencies {
            persistence: ryframe_db::application_ports::tenant_config::transfer(database.clone()),
            queue: queue.clone(),
            user,
            file,
            product,
            authorization_cache: authorization_cache.clone(),
            archive: process_tenant_config_archive::codec(),
        },
        ryframe_application::system::TenantConfigTransferSettings {
            target_catalog: ryframe_api::tenant_config_target_catalog()?,
            config: application_policies.tenant_config_transfer,
        },
    ));
    let tenant_data_migration = Arc::new(TenantDataMigrationService::new(
        ryframe_tenant_db::application_ports::tenant_data::tracking(database.clone()),
        Arc::<ryframe_tenant_db::TenantDatabaseRouter>::clone(&tenant_data),
        Arc::<ryframe_tenant_db::TenantDatabaseRouter>::clone(&tenant_data),
        queue.clone(),
        authorization_cache.clone(),
    ));
    let execution_tenant_scope =
        process_jobs::execution_tenant_scope(application_policies.multi_tenancy);
    let worker = process_jobs::build_job_worker(
        queue.clone(),
        &application_policies.job_worker,
        execution_tenant_scope.clone(),
        process_jobs::JobWorkerDependencies {
            export: export.clone(),
            message: message.clone(),
            data_retention,
            user_import,
            tenant_config_transfer,
            tenant_data_migration,
            redis: redis.clone(),
            messaging_enabled: application_policies.messaging.enabled(),
        },
    )?;
    let schedules = if application_policies.job_schedule.enabled {
        let schedule_targets =
            process_jobs::build_schedule_targets(application_policies.messaging.enabled())?;
        process_jobs::validate_schedule_targets(&worker, &schedule_targets)?;
        Some(Arc::new(
            JobScheduleService::new(
                ryframe_db::application_ports::jobs::schedule(database.clone()),
                queue.clone(),
                execution_tenant_scope.clone(),
                schedule_targets,
                application_policies.job_schedule,
            )
            .with_metrics_observer(process_jobs::build_schedule_metrics_observer()),
        ))
    } else {
        None
    };

    if run_mode == process_startup::WorkerRunMode::Once {
        let scheduled = if let Some(schedules) = schedules.as_ref() {
            schedules.scan_due_once().await?
        } else {
            0
        };
        let outbox_worker = OutboxWorker::new(
            queue,
            Arc::clone(&outbox_persistence),
            &application_policies.job_worker,
            execution_tenant_scope.clone(),
        )?
        .with_authorization_cache(authorization_cache.clone());
        let outbox_result = outbox_worker.run_once("ryframe-worker-once-outbox").await?;
        let job_result = worker.run_once("ryframe-worker-once-job").await?;
        tracing::info!(
            scheduled,
            ?outbox_result,
            ?job_result,
            "Worker 单次运行已完成"
        );
        _telemetry_guard.shutdown();
        return Ok(());
    }

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let mut health_tasks = start_health_server(
        database,
        redis,
        config
            .redis
            .as_ref()
            .is_some_and(|item| item.mode == RedisMode::Required),
        Arc::from(config.monitor.metrics_bearer_token.as_str()),
        config.jobs.health_host.clone(),
        config.jobs.health_port,
        shutdown_receiver.clone(),
    )
    .await?;
    if run_mode == process_startup::WorkerRunMode::Probe {
        tracing::info!("Worker 候选依赖与健康探针已就绪；探活模式不消费后台任务");
        shutdown_signal(shutdown_sender.clone()).await;
        let _ = shutdown_sender.send(true);
        let shutdown_deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE_PERIOD;
        process_startup::wait_for_tasks_until(
            &mut health_tasks,
            "Worker 健康服务",
            shutdown_deadline,
        )
        .await;
        _telemetry_guard.shutdown();
        return Ok(());
    }
    let mut worker_tasks = worker.spawn(shutdown_receiver.clone());
    if let Some(schedules) = schedules {
        worker_tasks.push(schedules.spawn(shutdown_receiver.clone()));
    } else {
        tracing::info!("Cron 调度已关闭，独立 Worker 仅消费普通后台任务");
    }
    worker_tasks.extend(
        OutboxWorker::new(
            queue.clone(),
            outbox_persistence,
            &application_policies.job_worker,
            execution_tenant_scope,
        )?
        .with_authorization_cache(authorization_cache)
        .spawn(shutdown_receiver),
    );
    tracing::info!(
        concurrency = config.jobs.concurrency,
        "独立后台任务 Worker 已启动"
    );
    shutdown_signal(shutdown_sender.clone()).await;
    let _ = shutdown_sender.send(true);

    let shutdown_deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE_PERIOD;
    process_startup::wait_for_task_groups_until(
        &mut worker_tasks,
        "后台任务 Worker",
        &mut health_tasks,
        "Worker 健康服务",
        shutdown_deadline,
    )
    .await;
    _telemetry_guard.shutdown();
    Ok(())
}

#[path = "ryframe_worker/health.rs"]
mod health;
#[path = "ryframe_worker/runtime.rs"]
mod runtime;

use health::start_health_server;
use runtime::{
    connect_redis_for_worker, connect_storage_for_worker, install_database_metrics,
    install_job_metrics, shutdown_signal,
};
