use std::{collections::BTreeMap, sync::Arc};

use crate::{
    ControlDatabaseCluster, DataRetentionRepository,
    RetentionCleanupResult as DatabaseCleanupResult, RetentionCutoff as DatabaseCutoff,
    RetentionResource as DatabaseResource, UserImportRepository,
};
use chrono::{DateTime, Utc};

use ryframe_application::{
    ExpiredImportArtifact, PersistenceFuture, RetentionCleanupPersistencePort,
    RetentionCleanupResult, RetentionCutoff, RetentionResource,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn RetentionCleanupPersistencePort> {
    Arc::new(DatabaseRetentionCleanupPersistence { database })
}

struct DatabaseRetentionCleanupPersistence {
    database: ControlDatabaseCluster,
}

impl RetentionCleanupPersistencePort for DatabaseRetentionCleanupPersistence {
    fn preview<'a>(
        &'a self,
        cutoffs: &'a [RetentionCutoff],
        current_run_id: Option<i64>,
    ) -> PersistenceFuture<'a, BTreeMap<String, u64>> {
        Box::pin(async move {
            let database_cutoffs = cutoffs
                .iter()
                .copied()
                .map(to_database_cutoff)
                .collect::<Vec<_>>();
            DataRetentionRepository
                .preview(self.database.write(), &database_cutoffs, current_run_id)
                .await
        })
    }

    fn cleanup_resource(
        &self,
        cutoff: RetentionCutoff,
        batch_size: usize,
        maximum: usize,
        current_run_id: Option<i64>,
    ) -> PersistenceFuture<'_, RetentionCleanupResult> {
        Box::pin(async move {
            DataRetentionRepository
                .cleanup_resource(
                    self.database.write(),
                    to_database_cutoff(cutoff),
                    batch_size,
                    maximum,
                    current_run_id,
                )
                .await
                .map(from_database_result)
        })
    }

    fn count_expired_import_artifacts(&self, before: DateTime<Utc>) -> PersistenceFuture<'_, u64> {
        Box::pin(async move {
            UserImportRepository
                .count_expired_artifacts(self.database.write(), before)
                .await
        })
    }

    fn list_expired_import_artifacts(
        &self,
        before: DateTime<Utc>,
        after_id: Option<i64>,
        limit: usize,
    ) -> PersistenceFuture<'_, Vec<ExpiredImportArtifact>> {
        Box::pin(async move {
            UserImportRepository
                .list_expired_artifacts_after_id(self.database.write(), before, after_id, limit)
                .await
                .map(|artifacts| {
                    artifacts
                        .into_iter()
                        .map(|artifact| ExpiredImportArtifact {
                            tenant_id: artifact.tenant_id,
                            file_id: artifact.file_id,
                        })
                        .collect()
                })
        })
    }
}

fn to_database_cutoff(cutoff: RetentionCutoff) -> DatabaseCutoff {
    DatabaseCutoff {
        resource: to_database_resource(cutoff.resource),
        before: cutoff.before,
    }
}

const fn to_database_resource(resource: RetentionResource) -> DatabaseResource {
    match resource {
        RetentionResource::BackgroundJobs => DatabaseResource::BackgroundJobs,
        RetentionResource::OutboxEvents => DatabaseResource::OutboxEvents,
        RetentionResource::ScheduleExecutions => DatabaseResource::ScheduleExecutions,
        RetentionResource::ExportJobs => DatabaseResource::ExportJobs,
        RetentionResource::OperationLogs => DatabaseResource::OperationLogs,
        RetentionResource::LoginLogs => DatabaseResource::LoginLogs,
        RetentionResource::UserImports => DatabaseResource::UserImports,
        RetentionResource::ServiceAccessAudits => DatabaseResource::ServiceAccessAudits,
        RetentionResource::RetentionRuns => DatabaseResource::RetentionRuns,
    }
}

const fn from_database_result(result: DatabaseCleanupResult) -> RetentionCleanupResult {
    RetentionCleanupResult {
        deleted: result.deleted,
        remaining: result.remaining,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_application_resource_maps_to_the_same_database_key() {
        for resource in RetentionResource::ALL {
            assert_eq!(resource.key(), to_database_resource(resource).key());
        }
    }
}
