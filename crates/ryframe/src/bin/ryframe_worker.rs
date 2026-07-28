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
use ryframe_config::{AppConfig, MigrationMode, RedisMode, StorageBackend};
use ryframe_core::RedisClient;
use ryframe_db::{CallbackDatabaseMetricsObserver, DatabaseCluster, DbSpanLayer, SqlLogLayer};
use ryframe_http::AppError;
use ryframe_middleware::telemetry::{TelemetryGuard, init_tracer_provider};
use ryframe_service::{
    CallbackJobMetricsObserver, ExportCleanupJobHandler, ExportJobHandler, JobQueue, JobWorker,
    MessageDispatchJobHandler, MessageRetentionJobHandler, OperLogJobHandler, OutboxWorker,
    spawn_message_retention_scheduler,
    system::{EXPORT_BUCKET, ExportService, MessageService, OperLogService, UserService},
};
use ryframe_storage::{LocalObjectStorage, ObjectStorage, S3Config, S3ObjectStorage};
use tokio::sync::watch;
use tracing_subscriber::{
    EnvFilter, Layer, filter::FilterFn, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let run_once = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [command] if command == "--once" => true,
        _ => return Err(AppError::Config("用法: ryframe-worker [--once]".into())),
    };
    let config = AppConfig::load_from_env()?;
    let _telemetry_guard = init_logging(&config);
    ryframe_middleware::metrics::spawn_process_metrics_updater();

    let primary = ryframe_db::connection::connect_with_level(
        &config.database.primary,
        config.database.sql_log_level,
    )
    .await?;
    ryframe_db::connection::ping(&primary).await?;
    let database = DatabaseCluster::single(primary);
    install_database_metrics(&database);

    match config.database.migration_mode {
        MigrationMode::Auto => ryframe_db_migration::up(database.write())
            .await
            .map_err(|error| AppError::Database(format!("数据库迁移失败: {error}")))?,
        MigrationMode::Verify => ryframe_db_migration::verify(database.write())
            .await
            .map_err(|error| AppError::Database(format!("数据库迁移校验失败: {error}")))?,
        MigrationMode::Off => {
            tracing::warn!("测试环境已关闭数据库迁移校验");
        }
    }
    ryframe_db_migration::verify_current_schema(database.write())
        .await
        .map_err(|error| AppError::Internal(format!("数据库结构指纹校验失败: {error}")))?;

    let redis = connect_redis_for_worker(&config).await?;
    let object_storage = connect_storage_for_worker(&config).await?;

    let queue = Arc::new(JobQueue::new(database.clone()));
    install_job_metrics(&queue);
    let oper_log = Arc::new(OperLogService::new(database.clone()));
    let message = Arc::new(MessageService::new(database.clone(), queue.clone()));
    let user = Arc::new(UserService::new(database.clone(), redis.clone()));
    let export = Arc::new(ExportService::new(database.clone(), user, object_storage));
    let worker = JobWorker::new(queue.clone(), &config.jobs)?
        .with_handler(Arc::new(OperLogJobHandler::new(oper_log)))?
        .with_handler(Arc::new(ExportJobHandler::new(export.clone())))?
        .with_handler(Arc::new(ExportCleanupJobHandler::new(export.clone())))?
        .with_handler(Arc::new(
            MessageDispatchJobHandler::new(message.clone(), redis.clone())
                .with_redis_wakeup_failure_observer(Arc::new(|| {
                    ryframe_middleware::metrics::record_redis_degraded("message_dispatch_wakeup");
                })),
        ))?
        .with_handler(Arc::new(
            MessageRetentionJobHandler::new(message).with_deleted_observer(Arc::new(
                ryframe_middleware::metrics::record_message_retention_deleted,
            )),
        ))?;

    if run_once {
        let outbox_worker = OutboxWorker::new(queue, &config.jobs)?;
        let outbox_result = outbox_worker.run_once("ryframe-worker-once-outbox").await?;
        let job_result = worker.run_once("ryframe-worker-once-job").await?;
        tracing::info!(?outbox_result, ?job_result, "Worker 单次运行已完成");
        _telemetry_guard.shutdown();
        return Ok(());
    }

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let mut retention_scheduler =
        spawn_message_retention_scheduler(queue.clone(), shutdown_receiver.clone());
    let mut health_task = start_health_server(
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
    let mut worker_tasks = worker.spawn(shutdown_receiver.clone());
    worker_tasks.extend(OutboxWorker::new(queue.clone(), &config.jobs)?.spawn(shutdown_receiver));
    tracing::info!(
        concurrency = config.jobs.concurrency,
        "独立后台任务 Worker 已启动"
    );
    shutdown_signal(shutdown_sender.clone()).await;
    let _ = shutdown_sender.send(true);

    let worker_shutdown_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
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
    if tokio::time::timeout(std::time::Duration::from_secs(5), &mut health_task)
        .await
        .is_err()
    {
        tracing::warn!("Worker 健康探针未在宽限期内停止");
        health_task.abort();
    }
    _telemetry_guard.shutdown();
    Ok(())
}

#[derive(Clone)]
struct WorkerHealthState {
    database: DatabaseCluster,
    redis: Option<RedisClient>,
    redis_required: bool,
    metrics_bearer_token: Arc<str>,
}

/// 启动独立 Worker 的存活、就绪和 Prometheus 指标端点。
async fn start_health_server(
    database: DatabaseCluster,
    redis: Option<RedisClient>,
    redis_required: bool,
    metrics_bearer_token: Arc<str>,
    host: String,
    port: u16,
    mut shutdown: watch::Receiver<bool>,
) -> Result<tokio::task::JoinHandle<()>, AppError> {
    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|error| AppError::Config(format!("Worker 健康探针地址无效: {error}")))?;
    let state = WorkerHealthState {
        database,
        redis,
        redis_required,
        metrics_bearer_token,
    };
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| {
            AppError::Internal(format!("无法绑定 Worker 健康探针 {address}: {error}"))
        })?;
    let router = Router::new()
        .route("/livez", get(worker_livez))
        .route("/readyz", get(worker_readyz))
        .route("/metrics", get(worker_metrics))
        .with_state(state);
    tracing::info!(%address, "Worker 健康探针已启动");

    Ok(tokio::spawn(async move {
        let shutdown_signal = async move {
            loop {
                if shutdown.changed().await.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        };
        if let Err(error) = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal)
            .await
        {
            tracing::warn!(%error, "Worker 健康探针已停止");
        }
    }))
}

async fn worker_livez() -> StatusCode {
    StatusCode::OK
}

async fn worker_readyz(State(state): State<WorkerHealthState>) -> StatusCode {
    let dependency_timeout = Duration::from_secs(2);
    if !matches!(
        tokio::time::timeout(
            dependency_timeout,
            ryframe_db::connection::ping(state.database.write()),
        )
        .await,
        Ok(Ok(()))
    ) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    if state.redis_required {
        let Some(redis) = state.redis.as_ref() else {
            return StatusCode::SERVICE_UNAVAILABLE;
        };
        if !matches!(
            tokio::time::timeout(dependency_timeout, redis.ping()).await,
            Ok(Ok(_))
        ) {
            return StatusCode::SERVICE_UNAVAILABLE;
        }
    }
    StatusCode::OK
}

async fn worker_metrics(
    State(state): State<WorkerHealthState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !state.metrics_bearer_token.is_empty()
        && !has_valid_metrics_token(&headers, &state.metrics_bearer_token)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        ryframe_middleware::metrics::metrics_text(),
    )
        .into_response()
}

fn has_valid_metrics_token(headers: &HeaderMap, expected: &str) -> bool {
    let Some(actual) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    constant_time_eq(actual.as_bytes(), expected.as_bytes())
}

fn constant_time_eq(actual: &[u8], expected: &[u8]) -> bool {
    let mut difference = actual.len() ^ expected.len();
    for index in 0..actual.len().max(expected.len()) {
        difference |= usize::from(actual.get(index).copied().unwrap_or(0))
            ^ usize::from(expected.get(index).copied().unwrap_or(0));
    }
    difference == 0
}

/// 初始化 worker 的 Redis 连接；可选 Redis 故障只降级为收件箱补拉。
async fn connect_storage_for_worker(
    config: &AppConfig,
) -> Result<Arc<dyn ObjectStorage>, AppError> {
    let storage: Arc<dyn ObjectStorage> = match config.object_storage.backend {
        StorageBackend::Local => Arc::new(LocalObjectStorage::new(
            &config.object_storage.local_base_dir,
        )),
        StorageBackend::Rustfs | StorageBackend::Minio | StorageBackend::S3 => Arc::new(
            S3ObjectStorage::new(S3Config {
                endpoint: config.object_storage.endpoint.clone(),
                access_key: config.object_storage.access_key.clone(),
                secret_key: config.object_storage.secret_key.clone(),
                use_ssl: config.object_storage.use_ssl,
                region: config.object_storage.region.clone(),
            })
            .map_err(|error| AppError::Config(error.to_string()))?,
        ),
    };
    storage
        .ensure_bucket(EXPORT_BUCKET)
        .await
        .map_err(|error| {
            AppError::ServiceUnavailable(format!("Worker 导出对象存储不可用: {error}"))
        })?;
    Ok(storage)
}

async fn connect_redis_for_worker(config: &AppConfig) -> Result<Option<RedisClient>, AppError> {
    let Some(redis_config) = config.redis.as_ref() else {
        return Ok(None);
    };
    if redis_config.mode == RedisMode::Disabled {
        return Ok(None);
    }
    match RedisClient::connect(redis_config).await {
        Ok(client) => match client.ping().await {
            Ok(_) => Ok(Some(client)),
            Err(error) if redis_config.mode == RedisMode::Required => Err(
                AppError::ServiceUnavailable(format!("Worker Redis PING 失败: {error}")),
            ),
            Err(error) => {
                tracing::warn!(%error, "Worker Redis 可选 PING 失败，消息将通过收件箱补拉");
                Ok(None)
            }
        },
        Err(error) if redis_config.mode == RedisMode::Required => Err(
            AppError::ServiceUnavailable(format!("Worker Redis 连接失败: {error}")),
        ),
        Err(error) => {
            tracing::warn!(%error, "Worker Redis 可选连接失败，消息将通过收件箱补拉");
            Ok(None)
        }
    }
}

/// 初始化独立 Worker 的应用日志、SQL 日志和 OpenTelemetry 链路。
///
/// 返回的守卫必须存活至进程退出，确保已缓存的 Span 在关闭前完成导出。
fn init_logging(config: &AppConfig) -> TelemetryGuard {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logger.level));
    let sqlx_filter = FilterFn::new(|meta| meta.target() != "sqlx::query");

    let telemetry_guard = init_tracer_provider(&config.telemetry);
    let otel_layer = telemetry_guard.tracing_layer();
    let fmt_layer = if config.logger.format == "json" {
        fmt::layer()
            .json()
            .with_ansi(config.logger.output != "file")
            .with_filter(sqlx_filter)
            .boxed()
    } else {
        fmt::layer()
            .with_ansi(config.logger.output != "file")
            .with_filter(sqlx_filter)
            .boxed()
    };
    let subscriber = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(DbSpanLayer::new())
        .with(SqlLogLayer::new(config.database.sql_log_level, 0));

    if let Some(otel) = otel_layer {
        subscriber.with(otel).with(env_filter).init();
    } else {
        subscriber.with(env_filter).init();
    }

    telemetry_guard
}

/// 等待 Ctrl+C 或 Unix 的 SIGTERM，并通知所有消费循环退出。
async fn shutdown_signal(shutdown_sender: watch::Sender<bool>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("无法安装 Ctrl+C 信号处理器");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("无法安装 SIGTERM 信号处理器")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    tracing::info!("收到关闭信号，正在停止后台任务 Worker");
    let _ = shutdown_sender.send(true);
}

/// 在 Worker 进程边界将底层数据库事件绑定到 Prometheus 指标。
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

/// 在 Worker 进程边界将后台任务事件绑定到 Prometheus 指标。
fn install_job_metrics(queue: &JobQueue) {
    queue.set_metrics_observer(Arc::new(CallbackJobMetricsObserver::new(
        Arc::new(ryframe_middleware::metrics::set_job_queue_depth),
        Arc::new(ryframe_middleware::metrics::set_job_oldest_ready_age),
        Arc::new(ryframe_middleware::metrics::observe_job_duration),
    )));
}
