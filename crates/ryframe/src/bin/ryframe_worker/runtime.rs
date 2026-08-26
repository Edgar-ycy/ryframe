use super::*;

/// 初始化 worker 的 Redis 连接；可选 Redis 故障只降级为收件箱补拉。
pub(super) async fn connect_storage_for_worker(
    config: &AppConfig,
    allows_initialization_writes: bool,
) -> Result<Arc<dyn ArtifactStore>, AppError> {
    let raw_storage: Arc<dyn ObjectStorage> = match config.object_storage.backend {
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
                request_timeout_secs: config.object_storage.request_timeout_secs,
            })
            .map_err(|error| AppError::Config(error.to_string()))?,
        ),
    };
    let storage: Arc<dyn ObjectStorage> = Arc::new(ScopedObjectStorage::new(
        raw_storage,
        config.scope_id.as_str(),
    ));
    for bucket in [EXPORT_BUCKET, IMPORT_BUCKET, CONFIG_PACKAGE_BUCKET] {
        process_startup::provision_or_verify(
            allows_initialization_writes,
            || storage.ensure_bucket(bucket),
            || storage.readiness_check(bucket),
        )
        .await
        .map_err(|error| AppError::ServiceUnavailable(format!("Worker 对象存储不可用: {error}")))?;
    }
    Ok(process_artifact_store::application_store(storage))
}

pub(super) async fn connect_redis_for_worker(
    config: &AppConfig,
    allows_initialization_writes: bool,
) -> Result<Option<RedisClient>, AppError> {
    let Some(redis_config) = config.redis.as_ref() else {
        return Ok(None);
    };
    if redis_config.mode == RedisMode::Disabled {
        return Ok(None);
    }
    match RedisClient::connect(redis_config).await {
        Ok(client) => match client.ping().await {
            Ok(_) => {
                let ownership_marker = redis_config.scope_id().ownership_marker("redis");
                match process_startup::provision_or_verify(
                    allows_initialization_writes,
                    || client.ensure_scope_ownership(&ownership_marker),
                    || client.verify_scope_ownership(&ownership_marker),
                )
                .await
                {
                    Ok(()) => Ok(Some(client)),
                    Err(error) if redis_config.mode == RedisMode::Required => Err(
                        AppError::Config(format!("Worker Redis scope 所有权校验失败: {error}")),
                    ),
                    Err(error) => {
                        tracing::warn!(%error, "Worker Redis scope 所有权校验失败，消息将通过收件箱补拉");
                        Ok(None)
                    }
                }
            }
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
pub(super) async fn shutdown_signal(shutdown_sender: watch::Sender<bool>) {
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

/// 在 Worker 进程边界将后台任务事件绑定到 Prometheus 指标。
pub(super) fn install_job_metrics(queue: &JobQueue) {
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
