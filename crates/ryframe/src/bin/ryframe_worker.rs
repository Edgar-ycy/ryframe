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
use ryframe_config::{
    AppConfig, Environment, JobWorkerMode, MigrationMode, RedisMode, StorageBackend,
};
use ryframe_core::RedisClient;
use ryframe_db::{CallbackDatabaseMetricsObserver, DatabaseCluster};
use ryframe_kernel::AppError;
use ryframe_service::{
    AuthorizationCache, CallbackJobMetricsObserver, JobQueue, JobScheduleService, OutboxWorker,
    system::{
        DataRetentionService, EXPORT_BUCKET, ExportService, FileService, IMPORT_BUCKET,
        MessageService, OperLogService, UserImportService, UserService,
    },
};
use ryframe_storage::{LocalObjectStorage, ObjectStorage, S3Config, S3ObjectStorage};
use tokio::sync::watch;

#[path = "../boot/jobs.rs"]
mod process_jobs;
#[path = "../boot/logging.rs"]
mod process_logging;
#[path = "../boot/readiness.rs"]
mod process_readiness;

/// Worker 进程在收到关闭信号后的全部后台任务总宽限时间。
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<(), AppError> {
    ryframe_service::set_audit_failure_hook(ryframe_middleware::metrics::record_audit_failure);
    ryframe_service::set_authorization_cache_lookup_hook(
        ryframe_middleware::metrics::record_authorization_cache_lookup,
    );
    let run_once = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [command] if command == "--once" => true,
        _ => return Err(AppError::Config("用法: ryframe-worker [--once]".into())),
    };
    let environment = Environment::from_env()?;
    let config = AppConfig::load_from_env(environment)?;
    if config.jobs.mode != JobWorkerMode::External {
        return Err(AppError::Config(
            "ryframe-worker 仅在 jobs.mode = \"external\" 时运行；embedded 由 API 进程消费，disabled 不消费任务".into(),
        ));
    }
    ryframe_utils::snowflake::initialize(config.snowflake_worker_id)
        .map_err(|error| AppError::Config(format!("Snowflake 初始化失败: {error}")))?;
    let (_logger_guard, _telemetry_guard) = process_logging::init(&config)?;
    ryframe_middleware::metrics::spawn_process_metrics_updater();

    let primary = ryframe_db::connection::connect_with_sql_logging(
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
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
            tracing::warn!("隔离环境已关闭数据库迁移校验");
        }
    }
    ryframe_db_migration::verify_current_schema(database.write())
        .await
        .map_err(|error| AppError::Internal(format!("数据库结构指纹校验失败: {error}")))?;

    let redis = connect_redis_for_worker(&config).await?;
    let authorization_cache = AuthorizationCache::new(
        redis.clone(),
        config
            .redis
            .as_ref()
            .map(|redis| redis.mode)
            .unwrap_or(RedisMode::Disabled),
    );
    let object_storage = connect_storage_for_worker(&config).await?;

    let queue = Arc::new(JobQueue::new(database.clone()).with_wakeup_redis(redis.clone()));
    install_job_metrics(&queue);
    let oper_log = Arc::new(OperLogService::new(database.clone()));
    let message = Arc::new(MessageService::new(
        database.clone(),
        queue.clone(),
        config.messaging.clone(),
    ));
    let user = Arc::new(UserService::new(
        database.clone(),
        authorization_cache.clone(),
    ));
    let file = Arc::new(FileService::new(database.clone(), object_storage.clone()));
    file.spawn_upload_janitor();
    let export = Arc::new(
        ExportService::new(database.clone(), user.clone(), object_storage, &config.jobs)
            .with_job_queue(queue.clone()),
    );
    let data_retention = Arc::new(DataRetentionService::new(
        database.clone(),
        queue.clone(),
        file.clone(),
        config.data_retention.clone(),
    ));
    let user_import = Arc::new(UserImportService::new(
        database.clone(),
        queue.clone(),
        user,
        file,
        config.user_import.clone(),
    ));
    let worker = process_jobs::build_job_worker(
        queue.clone(),
        &config.jobs,
        process_jobs::JobWorkerDependencies {
            export: export.clone(),
            message: message.clone(),
            data_retention,
            user_import,
            redis: redis.clone(),
            messaging_enabled: config.messaging.enabled,
        },
    )?;
    let schedules = if config.jobs.scheduler_enabled {
        let schedule_targets = process_jobs::build_schedule_targets(config.messaging.enabled)?;
        process_jobs::validate_schedule_targets(&worker, &schedule_targets)?;
        Some(Arc::new(
            JobScheduleService::new(
                database.clone(),
                queue.clone(),
                schedule_targets,
                &config.jobs,
            )
            .with_metrics_observer(process_jobs::build_schedule_metrics_observer()),
        ))
    } else {
        None
    };

    if run_once {
        let scheduled = if let Some(schedules) = schedules.as_ref() {
            schedules.scan_due_once().await?
        } else {
            0
        };
        let outbox_worker = OutboxWorker::new(queue, &config.jobs)?
            .with_authorization_cache(authorization_cache.clone())
            .with_audit_service(oper_log.clone());
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
    let mut worker_tasks = worker.spawn(shutdown_receiver.clone());
    if let Some(schedules) = schedules {
        worker_tasks.push(schedules.spawn(shutdown_receiver.clone()));
    } else {
        tracing::info!("Cron 调度已关闭，独立 Worker 仅消费普通后台任务");
    }
    worker_tasks.extend(
        OutboxWorker::new(queue.clone(), &config.jobs)?
            .with_authorization_cache(authorization_cache)
            .with_audit_service(oper_log)
            .spawn(shutdown_receiver),
    );
    tracing::info!(
        concurrency = config.jobs.concurrency,
        "独立后台任务 Worker 已启动"
    );
    shutdown_signal(shutdown_sender.clone()).await;
    let _ = shutdown_sender.send(true);

    let shutdown_deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE_PERIOD;
    for task in &mut worker_tasks {
        if tokio::time::timeout_at(shutdown_deadline, &mut *task)
            .await
            .is_err()
        {
            tracing::warn!("后台任务 Worker 未在总宽限时间内退出，已中止");
            task.abort();
        }
    }
    for task in &mut health_tasks {
        if tokio::time::timeout_at(shutdown_deadline, &mut *task)
            .await
            .is_err()
        {
            tracing::warn!("Worker 健康服务未在总宽限期内停止");
            task.abort();
        }
    }
    _telemetry_guard.shutdown();
    Ok(())
}

#[derive(Clone)]
struct WorkerHealthState {
    readiness: ryframe_monitor::DependencyHealthCache,
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
) -> Result<Vec<tokio::task::JoinHandle<()>>, AppError> {
    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|error| AppError::Config(format!("Worker 健康探针地址无效: {error}")))?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| {
            AppError::Internal(format!("无法绑定 Worker 健康探针 {address}: {error}"))
        })?;
    let readiness = ryframe_monitor::DependencyHealthCache::new(
        redis_required,
        false,
        process_readiness::CACHE_MAX_AGE,
    );
    let readiness_task = process_readiness::spawn(
        Arc::new(ryframe_db::SeaOrmDatabaseMonitor::new(database)),
        redis,
        None,
        readiness.clone(),
        shutdown.clone(),
    );
    let state = WorkerHealthState {
        readiness,
        metrics_bearer_token,
    };
    let router = Router::new()
        .route("/livez", get(worker_livez))
        .route("/readyz", get(worker_readyz))
        .route("/metrics", get(worker_metrics))
        .with_state(state);
    tracing::info!(%address, "Worker 健康探针已启动");

    let server_task = tokio::spawn(async move {
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
    });
    Ok(vec![server_task, readiness_task])
}

async fn worker_livez() -> StatusCode {
    StatusCode::OK
}

async fn worker_readyz(State(state): State<WorkerHealthState>) -> StatusCode {
    if state.readiness.snapshot().is_ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
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
    for bucket in [EXPORT_BUCKET, IMPORT_BUCKET] {
        storage.ensure_bucket(bucket).await.map_err(|error| {
            AppError::ServiceUnavailable(format!("Worker 对象存储不可用: {error}"))
        })?;
    }
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

/// 等待 Ctrl+C、Unix 的 SIGTERM 或 Windows 的 Ctrl+Break，并通知所有消费循环退出。
async fn shutdown_signal(shutdown_sender: watch::Sender<bool>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("无法安装 Ctrl+C 信号处理器");
    };

    #[cfg(unix)]
    let platform_shutdown = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("无法安装 SIGTERM 信号处理器")
            .recv()
            .await;
    };

    #[cfg(windows)]
    let platform_shutdown = async {
        tokio::signal::windows::ctrl_break()
            .expect("无法安装 Ctrl+Break 信号处理器")
            .recv()
            .await;
    };

    #[cfg(not(any(unix, windows)))]
    let platform_shutdown = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = platform_shutdown => {}
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
        Arc::new(ryframe_middleware::metrics::record_job_claim_attempt),
        Arc::new(ryframe_middleware::metrics::record_job_wakeup),
        Arc::new(ryframe_middleware::metrics::set_job_wakeup_listener_up),
        Arc::new(ryframe_middleware::metrics::record_job_wakeup_protocol_error),
    )));
}
