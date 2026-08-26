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
use ryframe::boot::{
    application_policy as process_application_policy, artifact_store as process_artifact_store,
    background_services::{BackgroundServiceInfrastructure, build as build_background_services},
    control_plane::{self, ControlPlaneStartup},
    jobs as process_jobs, logging as process_logging, readiness as process_readiness,
    startup as process_startup,
};
use ryframe_adapters::RedisClient;
use ryframe_adapters::storage::{
    LocalObjectStorage, ObjectStorage, S3Config, S3ObjectStorage, ScopedObjectStorage,
};
use ryframe_api::monitor::DependencyHealthCache;
use ryframe_application::{
    CallbackJobMetricsObserver, JobQueue, OutboxWorker,
    ports::files::ArtifactStore,
    system::{
        content::{CONFIG_PACKAGE_BUCKET, IMPORT_BUCKET},
        operations::EXPORT_BUCKET,
    },
};
use ryframe_config::{AppConfig, Environment, JobWorkerMode, RedisMode, StorageBackend};
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::AppError;
use tokio::sync::watch;

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
    let prepared = control_plane::prepare(
        &database,
        &config,
        ControlPlaneStartup::worker(allows_initialization_writes),
    )
    .await?;
    let tenant_data = prepared.tenant_database_router;

    let redis = connect_redis_for_worker(&config, allows_initialization_writes).await?;
    let object_storage = connect_storage_for_worker(&config, allows_initialization_writes).await?;
    let background = build_background_services(
        &database,
        Arc::clone(&tenant_data),
        &application_policies,
        BackgroundServiceInfrastructure {
            redis_client: redis.clone(),
            object_storage,
            dict_cache: None,
            starts_background_tasks: run_mode != process_startup::WorkerRunMode::Probe,
        },
    )?;
    let queue = background.job_queue.clone();
    let outbox_persistence = ryframe_db::application_ports::jobs::outbox(database.clone());
    install_job_metrics(&queue);
    let execution_tenant_scope =
        process_jobs::execution_tenant_scope(application_policies.multi_tenancy);
    let worker = process_jobs::build_job_worker(
        queue.clone(),
        &application_policies.job_worker,
        execution_tenant_scope.clone(),
        process_jobs::JobWorkerDependencies::from_background_services(
            &background,
            redis.clone(),
            application_policies.messaging.enabled(),
        ),
    )?;
    let schedules = background.job_schedules.clone();
    if let Some(schedules) = schedules.as_ref() {
        process_jobs::validate_schedule_targets(&worker, schedules.target_registry())?;
    }
    let authorization_cache = background.authorization_cache;

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
    connect_redis_for_worker, connect_storage_for_worker, install_job_metrics, shutdown_signal,
};
