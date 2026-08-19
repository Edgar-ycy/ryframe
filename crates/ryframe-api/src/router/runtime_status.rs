use super::*;
use ryframe_config::{JobWorkerMode, StorageBackend};

pub(super) async fn probe_runtime_status(
    State(state): State<AppState>,
) -> HttpResult<Json<ApiResponse<RuntimeStatus>>> {
    let database_health = state.monitor.database.topology_health().await;
    let replicas_connected = database_health
        .replicas
        .iter()
        .all(|replica| replica.healthy);
    let healthy_replica_count = database_health
        .replicas
        .iter()
        .filter(|replica| replica.healthy)
        .count();
    let replicas = database_health
        .replicas
        .into_iter()
        .map(|replica| RuntimeDatabaseReplicaStatus {
            name: replica.name,
            connected: replica.healthy,
            consecutive_failures: replica.consecutive_failures,
            consecutive_successes: replica.consecutive_successes,
        })
        .collect::<Vec<_>>();
    let sources_connected = database_health.sources.iter().all(|source| source.healthy);
    let sources = database_health
        .sources
        .into_iter()
        .map(|source| RuntimeDatabaseSourceStatus {
            name: source.name,
            connected: source.healthy,
        })
        .collect::<Vec<_>>();
    let read_policy = match (replicas.len(), healthy_replica_count) {
        (0, _) => "primary",
        (_, 0) => "primary_fallback",
        _ => "round_robin",
    };
    let storage_connected = state.services.file.check_storage().await.is_ok();
    let storage_config = &state.config.object_storage;
    let read_selections = ryframe_adapters::metrics::database_read_selection_totals()
        .into_iter()
        .map(|(target, reason, count)| RuntimeDatabaseReadSelection {
            target: target.into(),
            reason: reason.into(),
            count,
        })
        .collect();

    Ok(Json(ApiResponse::success(RuntimeStatus {
        database: RuntimeDatabaseStatus {
            connected: database_health.primary_healthy && replicas_connected && sources_connected,
            driver: "mysql".into(),
            primary_connected: database_health.primary_healthy,
            replica_count: replicas.len(),
            replicas,
            source_count: sources.len(),
            sources,
            read_policy: read_policy.into(),
            read_fallback_total: ryframe_adapters::metrics::database_read_fallback_total(),
            read_selections,
        },
        redis: RuntimeRedisStatus {
            configured: state
                .config
                .redis
                .as_ref()
                .is_some_and(|config| config.mode != RedisMode::Disabled),
            connected: state.redis.is_some(),
        },
        object_storage: RuntimeStorageStatus {
            backend: storage_config.backend.as_str().into(),
            connected: storage_connected,
            endpoint: (storage_config.backend != StorageBackend::Local
                && !storage_config.endpoint.trim().is_empty())
            .then(|| storage_config.endpoint.clone()),
        },
        upload_circuit_breaker: RuntimeCircuitBreakerStatus {
            state: format!("{:?}", state.runtime.upload_circuit_breaker.current_state()),
        },
        jobs: RuntimeJobsStatus {
            mode: match state.config.jobs.mode {
                JobWorkerMode::Embedded => "embedded",
                JobWorkerMode::External => "external",
                JobWorkerMode::Disabled => "disabled",
            }
            .into(),
            scheduler_enabled: state.config.jobs.scheduler_enabled,
        },
    })))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RuntimeStatus {
    database: RuntimeDatabaseStatus,
    redis: RuntimeRedisStatus,
    object_storage: RuntimeStorageStatus,
    upload_circuit_breaker: RuntimeCircuitBreakerStatus,
    jobs: RuntimeJobsStatus,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeDatabaseStatus {
    connected: bool,
    driver: String,
    primary_connected: bool,
    replica_count: usize,
    replicas: Vec<RuntimeDatabaseReplicaStatus>,
    source_count: usize,
    sources: Vec<RuntimeDatabaseSourceStatus>,
    read_policy: String,
    read_fallback_total: u64,
    read_selections: Vec<RuntimeDatabaseReadSelection>,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeDatabaseReadSelection {
    target: String,
    reason: String,
    count: u64,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeDatabaseReplicaStatus {
    name: String,
    connected: bool,
    consecutive_failures: usize,
    consecutive_successes: usize,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeDatabaseSourceStatus {
    name: String,
    connected: bool,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeRedisStatus {
    configured: bool,
    connected: bool,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeStorageStatus {
    backend: String,
    connected: bool,
    endpoint: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeCircuitBreakerStatus {
    state: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeJobsStatus {
    mode: String,
    scheduler_enabled: bool,
}
