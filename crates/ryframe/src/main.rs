use std::{future::IntoFuture, net::SocketAddr, sync::Arc, time::Duration};

use ryframe::{app, boot};
use ryframe_adapters::i18n::LocalizerLoader;
use ryframe_application::{CallbackJobMetricsObserver, JobQueue, OutboxWorker};
use ryframe_config::{AppConfig, Environment, JobWorkerMode};
use ryframe_kernel::AppError;
use tokio::sync::{oneshot, watch};

/// API 进程在收到关闭信号后的全部后台任务总宽限时间。
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let run_mode =
        boot::startup::parse_api_run_mode(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let starts_background_tasks = run_mode.starts_background_tasks();
    ryframe_api::metrics::install(ryframe_api::metrics::ApiMetricsHooks {
        begin_http_request: ryframe_adapters::metrics::begin_http_request,
        finish_http_request: ryframe_adapters::metrics::finish_http_request,
        abandon_http_request: ryframe_adapters::metrics::abandon_http_request,
        metrics_text: ryframe_adapters::metrics::metrics_text,
        record_refresh_replay: ryframe_adapters::metrics::record_refresh_replay,
        record_csrf_rejection: ryframe_adapters::metrics::record_csrf_rejection,
        record_redis_degraded: ryframe_adapters::metrics::record_redis_degraded,
        record_idempotency_conflict: ryframe_adapters::metrics::record_idempotency_conflict,
        record_rate_limit_rejection: ryframe_adapters::metrics::record_rate_limit_rejection,
        record_ws_ticket: ryframe_adapters::metrics::record_ws_ticket,
        set_ws_connections: ryframe_adapters::metrics::set_ws_connections,
        record_message_delivery: ryframe_adapters::metrics::record_message_delivery,
        set_message_redis_listener_connected:
            ryframe_adapters::metrics::set_message_redis_listener_connected,
        record_message_replay_query: ryframe_adapters::metrics::record_message_replay_query,
        database_read_fallback_total: ryframe_adapters::metrics::database_read_fallback_total,
        database_read_selection_totals: ryframe_adapters::metrics::database_read_selection_totals,
        observe_message_ack_latency: ryframe_adapters::metrics::observe_message_ack_latency,
    })
    .map_err(|error| AppError::Internal(error.into()))?;
    ryframe_api::auth_middleware::set_backend_failure_hook(
        ryframe_adapters::metrics::record_redis_degraded,
    );
    ryframe_application::set_audit_failure_hook(ryframe_adapters::metrics::record_audit_failure);
    ryframe_application::set_authorization_cache_lookup_hook(
        ryframe_adapters::metrics::record_authorization_cache_lookup,
    );

    let environment = Environment::from_env()?;
    let config = AppConfig::load_from_env(environment)?;
    if run_mode == boot::startup::ApiRunMode::Probe && config.environment.is_production() {
        return Err(AppError::Config(
            "API 探活模式只允许本地开发和隔离检查使用".into(),
        ));
    }
    let application_policies = boot::application_policy::ApplicationPolicies::from_config(&config)?;
    ryframe_api::validate_runtime_features(config.api_docs.enabled)?;
    ryframe_adapters::snowflake::initialize(config.snowflake_worker_id)
        .map_err(|error| AppError::Config(format!("Snowflake 初始化失败: {error}")))?;
    ryframe_db::install_id_generator(|| {
        ryframe_adapters::snowflake::try_next_snowflake_id().map_err(AppError::from)
    })?;
    ryframe_application::install_id_generator(|| {
        ryframe_adapters::snowflake::try_next_snowflake_id().map_err(AppError::from)
    })?;
    let localizer = Arc::new(
        LocalizerLoader::load_from_environment(config.environment.is_production())
            .map_err(|error| AppError::Config(format!("国际化资源加载失败: {error}")))?,
    );
    let (_logger_guard, _telemetry_guard) = boot::logging::init(&config)?;
    tracing::info!(
        environment = %config.environment,
        "configuration loaded"
    );
    if starts_background_tasks {
        ryframe_adapters::metrics::spawn_process_metrics_updater();
    }

    let database = boot::datasource::connect(&config).await?;
    let control_plane = boot::control_plane::prepare(
        &database,
        &config,
        boot::control_plane::ControlPlaneStartup::api(starts_background_tasks),
    )
    .await?;
    let tenant_database_router = control_plane.tenant_database_router;
    let replica_health_monitor = starts_background_tasks.then(|| {
        boot::datasource::spawn_replica_health_monitor(
            database.clone(),
            config.database.replicas.clone(),
            config.database.sql_log_level,
            config.database.sql_slow_threshold_ms,
        )
    });

    let config_arc = Arc::new(config.clone());
    let redis =
        boot::redis::init(&config.redis, config.environment, starts_background_tasks).await?;
    let object_storage = boot::storage::init(&config, starts_background_tasks).await?;
    let limit = boot::limiter::init(&config, &redis.client, starts_background_tasks)?;
    let services = boot::services::build_all(
        &database,
        Arc::clone(&tenant_database_router),
        &config,
        &application_policies,
        boot::services::ServiceInfrastructure {
            redis_client: &redis.client,
            object_storage,
            rate_limiter: limit.limiter.clone(),
            starts_background_tasks,
        },
    )
    .await?;
    install_job_metrics(&services.operations.job_queue);
    let outbox_persistence = ryframe_db::application_ports::jobs::outbox(database.clone());

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let (server_info, mut server_info_sampler) =
        ryframe_adapters::monitor::ServerInfoSampler::spawn(shutdown_receiver.clone()).await?;
    let state = boot::app_state::assemble(boot::app_state::AppStateAssembly {
        database,
        config: config_arc,
        localizer,
        redis_client: redis.client.clone(),
        token_blacklist: redis.token_blacklist,
        services: services.clone(),
        limiter: limit.limiter.clone(),
        server_info,
    });
    let message_hub = state.message_hub.clone();
    let readiness_database = state.monitor.database.clone();
    let readiness_redis = redis.client.clone();
    let readiness_file = state.services.content.file.clone();
    let readiness_cache = state.monitor.readiness.clone();
    let message_listener = starts_background_tasks
        .then(|| {
            boot::message_listener::spawn(
                &state.message_hub,
                redis.client.clone(),
                services.content.message.clone(),
                services.platform.tenant_data.clone(),
                state.settings.messaging.enabled,
            )
        })
        .flatten();
    let mut message_replay_scheduler = starts_background_tasks
        .then(|| {
            state
                .message_hub
                .spawn_replay_scheduler(services.content.message.clone(), shutdown_receiver.clone())
        })
        .flatten();
    let router = app::build_app(state, limit.rate_limit_state)?;

    let addr = format!("{}:{}", config.app.host, config.app.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|error| AppError::Internal(format!("failed to bind {addr}: {error}")))?;
    tracing::info!(address = %addr, "HTTP server started");

    let mut readiness_monitor = boot::readiness::spawn(
        readiness_database,
        readiness_redis,
        Some(readiness_file),
        readiness_cache,
        shutdown_receiver.clone(),
    );
    let mut worker_tasks = if !starts_background_tasks {
        tracing::info!("API 候选探活模式不启动消息、清理、回放或后台任务循环");
        Vec::new()
    } else {
        match config.jobs.mode {
            JobWorkerMode::Embedded => {
                let execution_tenant_scope =
                    boot::jobs::execution_tenant_scope(application_policies.multi_tenancy);
                let worker = boot::jobs::build_job_worker(
                    services.operations.job_queue.clone(),
                    &application_policies.job_worker,
                    execution_tenant_scope.clone(),
                    boot::jobs::JobWorkerDependencies::from_api_services(
                        &services,
                        redis.client.clone(),
                        application_policies.messaging.enabled(),
                    ),
                )?;
                if let Some(schedules) = services.operations.job_schedules.as_ref() {
                    boot::jobs::validate_schedule_targets(&worker, schedules.target_registry())?;
                }
                tracing::info!(
                    concurrency = config.jobs.concurrency,
                    "已启动内置后台任务 Worker"
                );
                let mut tasks = worker.spawn(shutdown_receiver.clone());
                if let Some(schedules) = services.operations.job_schedules.clone() {
                    tasks.push(schedules.spawn(shutdown_receiver.clone()));
                } else {
                    tracing::info!("Cron 调度已关闭，内置 Worker 仅消费普通后台任务");
                }
                let authorization_cache = boot::authorization_cache::cache(
                    redis.client.clone(),
                    application_policies.cache,
                );
                tasks.extend(
                    OutboxWorker::new(
                        services.operations.job_queue.clone(),
                        outbox_persistence,
                        &application_policies.job_worker,
                        execution_tenant_scope,
                    )?
                    .with_authorization_cache(authorization_cache)
                    .spawn(shutdown_receiver),
                );
                tasks
            }
            JobWorkerMode::External => {
                tracing::info!("后台任务由独立 ryframe-worker 进程消费");
                Vec::new()
            }
            JobWorkerMode::Disabled => {
                tracing::warn!("后台任务 Worker 已禁用，仅应在隔离环境使用");
                Vec::new()
            }
        }
    };

    let (shutdown_deadline_sender, mut shutdown_deadline_receiver) = oneshot::channel();
    let (result, shutdown_deadline) = {
        let server = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal(
            shutdown_sender.clone(),
            message_hub.clone(),
            shutdown_deadline_sender,
        ))
        .into_future();
        tokio::pin!(server);

        tokio::select! {
            server_result = &mut server => (
                server_result.map_err(|error| {
                    AppError::Internal(format!("HTTP server stopped unexpectedly: {error}"))
                }),
                tokio::time::Instant::now() + SHUTDOWN_GRACE_PERIOD,
            ),
            received_deadline = &mut shutdown_deadline_receiver => {
                let shutdown_deadline = received_deadline
                    .unwrap_or_else(|_| tokio::time::Instant::now() + SHUTDOWN_GRACE_PERIOD);
                let result = match tokio::time::timeout_at(shutdown_deadline, &mut server).await {
                    Ok(server_result) => server_result.map_err(|error| {
                        AppError::Internal(format!("HTTP server stopped unexpectedly: {error}"))
                    }),
                    Err(_) => {
                        tracing::warn!("HTTP 服务未在总宽限时间内停止，已取消等待");
                        Ok(())
                    }
                };
                (result, shutdown_deadline)
            }
        }
    };

    message_hub.shutdown_all();
    let _ = shutdown_sender.send(true);
    for task in &mut worker_tasks {
        if tokio::time::timeout_at(shutdown_deadline, &mut *task)
            .await
            .is_err()
        {
            tracing::warn!("后台任务 Worker 未在总宽限时间内退出，已中止");
            task.abort();
        }
    }
    if tokio::time::timeout_at(shutdown_deadline, &mut readiness_monitor)
        .await
        .is_err()
    {
        tracing::warn!("后台就绪探测未在宽限期内停止");
        readiness_monitor.abort();
    }
    if tokio::time::timeout_at(shutdown_deadline, &mut server_info_sampler)
        .await
        .is_err()
    {
        tracing::warn!("服务器信息采样器未在宽限期内停止");
        server_info_sampler.abort();
    }
    if let Some(scheduler) = message_replay_scheduler.as_mut()
        && tokio::time::timeout_at(shutdown_deadline, &mut *scheduler)
            .await
            .is_err()
    {
        tracing::warn!("消息共享补拉调度器未在宽限期内停止");
        scheduler.abort();
    }
    if let Some(replica_health_monitor) = replica_health_monitor {
        replica_health_monitor.abort();
    }
    if let Some(listener) = message_listener {
        listener.abort();
    }
    tracing::info!("HTTP server stopped");
    result
}

async fn shutdown_signal(
    shutdown_sender: watch::Sender<bool>,
    message_hub: Arc<ryframe_api::message_socket::MessageHub>,
    shutdown_deadline_sender: oneshot::Sender<tokio::time::Instant>,
) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let platform_shutdown = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(windows)]
    let platform_shutdown = async {
        tokio::signal::windows::ctrl_break()
            .expect("failed to install Ctrl+Break handler")
            .recv()
            .await;
    };

    #[cfg(not(any(unix, windows)))]
    let platform_shutdown = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = platform_shutdown => {},
    }

    let shutdown_deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE_PERIOD;
    let _ = shutdown_deadline_sender.send(shutdown_deadline);
    message_hub.shutdown_all();
    let _ = shutdown_sender.send(true);
    tracing::info!("shutdown signal received");
}

/// 在应用边界将后台任务队列事件绑定到 Prometheus 指标。
fn install_job_metrics(queue: &JobQueue) {
    queue.set_metrics_observer(Arc::new(CallbackJobMetricsObserver::new(
        Arc::new(ryframe_adapters::metrics::set_job_queue_depth),
        Arc::new(ryframe_adapters::metrics::set_job_oldest_ready_age),
        Arc::new(ryframe_adapters::metrics::observe_job_duration),
        Arc::new(ryframe_adapters::metrics::record_job_claim_attempt),
        Arc::new(ryframe_adapters::metrics::record_job_wakeup),
        Arc::new(ryframe_adapters::metrics::set_job_wakeup_listener_up),
        Arc::new(ryframe_adapters::metrics::record_job_wakeup_protocol_error),
    )));
}
