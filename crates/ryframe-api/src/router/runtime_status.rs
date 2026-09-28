use super::*;
use ryframe_application::ports::backup::{BackupCollectionStatus, BackupHealthSnapshot};

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
    let storage_connected = state.services.content.file.check_storage().await.is_ok();
    let storage_config = &state.settings.object_storage;
    let read_selections = crate::metrics::database_read_selection_totals()
        .into_iter()
        .map(|(target, reason, count)| RuntimeDatabaseReadSelection {
            target: target.into(),
            reason: reason.into(),
            count,
        })
        .collect();
    let backup = runtime_backup_status(&state.monitor.backup_health.snapshot());

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
            read_fallback_total: crate::metrics::database_read_fallback_total(),
            read_selections,
        },
        redis: RuntimeRedisStatus {
            configured: state.settings.redis_configured,
            connected: state.redis_connected,
        },
        object_storage: RuntimeStorageStatus {
            backend: storage_config.backend.as_str().into(),
            connected: storage_connected,
            endpoint: storage_config.endpoint.clone(),
        },
        upload_circuit_breaker: RuntimeCircuitBreakerStatus {
            state: state
                .runtime
                .upload_circuit_breaker
                .state_label()
                .to_owned(),
        },
        jobs: RuntimeJobsStatus {
            mode: state.settings.jobs.mode.clone(),
            scheduler_enabled: state.settings.jobs.scheduler_enabled,
        },
        backup,
    })))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RuntimeStatus {
    database: RuntimeDatabaseStatus,
    redis: RuntimeRedisStatus,
    object_storage: RuntimeStorageStatus,
    upload_circuit_breaker: RuntimeCircuitBreakerStatus,
    jobs: RuntimeJobsStatus,
    backup: RuntimeBackupStatus,
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

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeBackupStatus {
    collector_status: RuntimeBackupCollectionStatus,
    available: bool,
    last_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
    last_success_at: Option<chrono::DateTime<chrono::Utc>>,
    health: Option<RuntimeBackupHealth>,
}

#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum RuntimeBackupCollectionStatus {
    Unknown,
    Available,
    Unavailable,
    Stale,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeBackupHealth {
    required_resources: u64,
    missing_resources: u64,
    expired_resources: u64,
    invalid_resources: u64,
    oldest_capture: Option<chrono::DateTime<chrono::Utc>>,
    last_restore_completed: Option<chrono::DateTime<chrono::Utc>>,
    last_restore_succeeded: bool,
    restore_duration_seconds: Option<i64>,
    recovery_point_age_seconds: Option<i64>,
    restore_running: u64,
    restore_overdue: u64,
}

fn runtime_backup_status(snapshot: &BackupHealthSnapshot) -> RuntimeBackupStatus {
    RuntimeBackupStatus {
        collector_status: match snapshot.status {
            BackupCollectionStatus::Unknown => RuntimeBackupCollectionStatus::Unknown,
            BackupCollectionStatus::Available => RuntimeBackupCollectionStatus::Available,
            BackupCollectionStatus::Unavailable => RuntimeBackupCollectionStatus::Unavailable,
            BackupCollectionStatus::Stale => RuntimeBackupCollectionStatus::Stale,
        },
        available: snapshot.available(),
        last_attempt_at: snapshot.last_attempt_at,
        last_success_at: snapshot.last_success_at,
        health: snapshot
            .last_valid_health
            .as_ref()
            .map(|health| RuntimeBackupHealth {
                required_resources: health.required_resources,
                missing_resources: health.missing_resources,
                expired_resources: health.expired_resources,
                invalid_resources: health.invalid_resources,
                oldest_capture: health.oldest_capture,
                last_restore_completed: health.last_restore_completed,
                last_restore_succeeded: health.last_restore_succeeded,
                restore_duration_seconds: health.restore_duration_seconds,
                recovery_point_age_seconds: health.recovery_point_age_seconds,
                restore_running: health.restore_running,
                restore_overdue: health.restore_overdue,
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use ryframe_application::ports::backup::{BackupHealth, BackupHealthCache};
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    #[test]
    fn backup_runtime_status_never_marks_unknown_or_failed_cache_available() {
        let started_at = Instant::now();
        let now = Arc::new(Mutex::new(started_at));
        let clock = Arc::clone(&now);
        let cache =
            BackupHealthCache::with_clock(Duration::from_secs(150), move || *clock.lock().unwrap());
        let unknown = runtime_backup_status(&cache.snapshot());
        assert_eq!(
            unknown.collector_status,
            RuntimeBackupCollectionStatus::Unknown
        );
        assert!(!unknown.available);
        assert!(unknown.health.is_none());

        let collected = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        cache.update_success(
            BackupHealth {
                required_resources: 6,
                missing_resources: 1,
                restore_running: 1,
                ..Default::default()
            },
            collected,
        );
        let available = runtime_backup_status(&cache.snapshot());
        assert_eq!(
            available.collector_status,
            RuntimeBackupCollectionStatus::Available
        );
        assert!(available.available);
        assert_eq!(available.last_success_at, Some(collected));
        let health = available.health.unwrap();
        assert_eq!(health.required_resources, 6);
        assert_eq!(health.missing_resources, 1);
        assert_eq!(health.restore_running, 1);

        cache.update_failure(collected + chrono::Duration::seconds(60));
        let unavailable = runtime_backup_status(&cache.snapshot());
        assert_eq!(
            unavailable.collector_status,
            RuntimeBackupCollectionStatus::Unavailable
        );
        assert!(!unavailable.available);
        assert_eq!(unavailable.last_success_at, Some(collected));
        assert_eq!(unavailable.health.unwrap().required_resources, 6);

        *now.lock().unwrap() = started_at + Duration::from_secs(151);
        let stale = runtime_backup_status(&cache.snapshot());
        assert_eq!(stale.collector_status, RuntimeBackupCollectionStatus::Stale);
        assert!(!stale.available);
        assert_eq!(stale.last_success_at, Some(collected));
        assert_eq!(stale.health.unwrap().required_resources, 6);
    }
}
