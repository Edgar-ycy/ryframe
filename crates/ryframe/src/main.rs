mod app;
mod boot;

use std::{net::SocketAddr, sync::Arc};

use ryframe_config::{AppConfig, Environment, JobWorkerMode, MigrationMode, RedisMode};
use ryframe_db::{CallbackDatabaseMetricsObserver, DatabaseCluster};
use ryframe_i18n::Localizer;
use ryframe_kernel::AppError;
use ryframe_service::{
    AuthorizationCache, CallbackJobMetricsObserver, ExportCleanupJobHandler, ExportJobHandler,
    JobQueue, JobWorker, MessageDispatchJobHandler, MessageRetentionJobHandler, OutboxWorker,
    spawn_message_retention_scheduler,
};
use tokio::sync::watch;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    ryframe_auth::middleware::set_backend_failure_hook(
        ryframe_middleware::metrics::record_redis_degraded,
    );
    ryframe_service::set_audit_failure_hook(ryframe_middleware::metrics::record_audit_failure);

    let environment = Environment::from_env()?;
    let config = AppConfig::load_from_env(environment)?;
    ryframe_utils::snowflake::initialize(config.snowflake_worker_id)
        .map_err(|error| AppError::Config(format!("Snowflake 初始化失败: {error}")))?;
    let localizer = Arc::new(
        Localizer::load_from_environment(config.environment.is_production())
            .map_err(|error| AppError::Config(format!("国际化资源加载失败: {error}")))?,
    );
    let (_logger_guard, _telemetry_guard) = boot::logging::init(&config)?;
    tracing::info!(
        environment = %config.environment,
        "configuration loaded"
    );
    ryframe_middleware::metrics::spawn_process_metrics_updater();

    let database = boot::datasource::connect(&config).await?;
    install_database_metrics(&database);
    match config.database.migration_mode {
        MigrationMode::Auto => ryframe_db_migration::up(database.write())
            .await
            .map_err(|error| AppError::Database(format!("database migration failed: {error}")))?,
        MigrationMode::Verify => ryframe_db_migration::verify(database.write())
            .await
            .map_err(|error| {
                AppError::Database(format!("database migration verification failed: {error}"))
            })?,
        MigrationMode::Off => {
            tracing::warn!("database migration checks are disabled for the test environment");
        }
    }
    boot::datasource::verify_schema(&database).await?;
    let replica_health_monitor = boot::datasource::spawn_replica_health_monitor(
        database.clone(),
        config.database.replicas.clone(),
        config.database.sql_log_level,
    );

    let config_arc = Arc::new(config.clone());
    let redis = boot::redis::init(&config.redis, config.environment).await?;
    let object_storage = boot::storage::init(&config).await?;
    let services =
        boot::services::build_all(&database, &config, &redis.client, object_storage).await?;
    install_job_metrics(&services.job_queue);
    let limit = boot::limiter::init(&config, &redis.client)?;

    let state = boot::app_state::assemble(
        database,
        config_arc,
        localizer,
        redis.client.clone(),
        redis.token_blacklist,
        services.clone(),
        limit.limiter.clone(),
    );
    let readiness_database = state.monitor.database.clone();
    let readiness_redis = state.monitor.redis.clone();
    let readiness_file_service = state.services.file.clone();
    let readiness_cache = state.monitor.readiness.clone();
    let message_listener = state
        .message_hub
        .spawn_redis_listener(redis.client.clone(), services.message.clone());
    let router = app::build_app(state, limit.rate_limit_state, &config.cors)?;

    let addr = format!("{}:{}", config.app.host, config.app.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|error| AppError::Internal(format!("failed to bind {addr}: {error}")))?;
    tracing::info!(address = %addr, "HTTP server started");

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let mut readiness_monitor = boot::readiness::spawn(
        readiness_database,
        readiness_redis,
        Some(readiness_file_service),
        readiness_cache,
        shutdown_receiver.clone(),
    );
    let mut retention_scheduler = spawn_message_retention_scheduler(
        services.job_queue.clone(),
        config.messaging.enabled,
        shutdown_receiver.clone(),
    );
    let mut worker_tasks = match config.jobs.mode {
        JobWorkerMode::Embedded => {
            let worker = JobWorker::new(services.job_queue.clone(), &config.jobs)?
                .with_handler(Arc::new(ExportJobHandler::new(services.export.clone())))?
                .with_handler(Arc::new(ExportCleanupJobHandler::new(
                    services.export.clone(),
                )))?;
            let worker = if config.messaging.enabled {
                worker
                    .with_handler(Arc::new(
                        MessageDispatchJobHandler::new(
                            services.message.clone(),
                            redis.client.clone(),
                        )
                        .with_redis_wakeup_failure_observer(Arc::new(
                            || {
                                ryframe_middleware::metrics::record_redis_degraded(
                                    "message_dispatch_wakeup",
                                );
                            },
                        )),
                    ))?
                    .with_handler(Arc::new(
                        MessageRetentionJobHandler::new(services.message.clone())
                            .with_deleted_observer(Arc::new(
                                ryframe_middleware::metrics::record_message_retention_deleted,
                            )),
                    ))?
            } else {
                worker
            };
            tracing::info!(
                concurrency = config.jobs.concurrency,
                "已启动内置后台任务 Worker"
            );
            let mut tasks = worker.spawn(shutdown_receiver.clone());
            let authorization_cache = AuthorizationCache::new(
                redis.client.clone(),
                config
                    .redis
                    .as_ref()
                    .map(|redis| redis.mode)
                    .unwrap_or(RedisMode::Disabled),
            );
            tasks.extend(
                OutboxWorker::new(services.job_queue.clone(), &config.jobs)?
                    .with_authorization_cache(authorization_cache)
                    .with_audit_service(services.oper_log.clone())
                    .spawn(shutdown_receiver),
            );
            tasks
        }
        JobWorkerMode::External => {
            tracing::info!("后台任务由独立 ryframe-worker 进程消费");
            Vec::new()
        }
        JobWorkerMode::Disabled => {
            tracing::warn!("后台任务 Worker 已禁用，仅应在测试环境使用");
            Vec::new()
        }
    };

    let result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(shutdown_sender.clone()))
    .await
    .map_err(|error| AppError::Internal(format!("HTTP server stopped unexpectedly: {error}")));

    let _ = shutdown_sender.send(true);
    let worker_shutdown_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    for task in &mut worker_tasks {
        if tokio::time::timeout_at(worker_shutdown_deadline, &mut *task)
            .await
            .is_err()
        {
            tracing::warn!("后台任务 Worker 未在总宽限时间内退出，已中止");
            task.abort();
        }
    }
    if tokio::time::timeout(std::time::Duration::from_secs(5), &mut retention_scheduler)
        .await
        .is_err()
    {
        tracing::warn!("消息保留调度器未在宽限期内停止");
        retention_scheduler.abort();
    }
    if tokio::time::timeout(std::time::Duration::from_secs(5), &mut readiness_monitor)
        .await
        .is_err()
    {
        tracing::warn!("后台就绪探测未在宽限期内停止");
        readiness_monitor.abort();
    }
    replica_health_monitor.abort();
    if let Some(listener) = message_listener {
        listener.abort();
    }
    tracing::info!("HTTP server stopped");
    result
}

async fn shutdown_signal(shutdown_sender: watch::Sender<bool>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
    let _ = shutdown_sender.send(true);
}

/// 在应用边界将底层数据库事件绑定到 Prometheus 指标。
fn install_database_metrics(database: &DatabaseCluster) {
    database.set_metrics_observer(Arc::new(CallbackDatabaseMetricsObserver::new(
        Arc::new(|kind, name, healthy| {
            ryframe_middleware::metrics::set_database_node_health(
                name,
                kind.metric_label(),
                healthy,
            );
        }),
        Arc::new(|target, reason| {
            ryframe_middleware::metrics::record_database_read_selection(
                target.metric_label(),
                reason.metric_label(),
            );
        }),
        Arc::new(ryframe_middleware::metrics::record_database_read_fallback),
    )));
}

/// 在应用边界将后台任务队列事件绑定到 Prometheus 指标。
fn install_job_metrics(queue: &JobQueue) {
    queue.set_metrics_observer(Arc::new(CallbackJobMetricsObserver::new(
        Arc::new(ryframe_middleware::metrics::set_job_queue_depth),
        Arc::new(ryframe_middleware::metrics::set_job_oldest_ready_age),
        Arc::new(ryframe_middleware::metrics::observe_job_duration),
    )));
}
