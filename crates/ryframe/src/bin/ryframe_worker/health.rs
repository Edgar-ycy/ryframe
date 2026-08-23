use super::*;

#[derive(Clone)]
struct WorkerHealthState {
    readiness: DependencyHealthCache,
    metrics_bearer_token: Arc<str>,
}

/// 启动独立 Worker 的存活、就绪和 Prometheus 指标端点。
pub(super) async fn start_health_server(
    database: ControlDatabaseCluster,
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
    let readiness =
        DependencyHealthCache::new(redis_required, false, process_readiness::CACHE_MAX_AGE);
    let readiness_task = process_readiness::spawn(
        process_readiness::database_monitor(database),
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
        ryframe_adapters::metrics::metrics_text(),
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
    ryframe_auth::constant_time_eq(actual.as_bytes(), expected.as_bytes())
}
