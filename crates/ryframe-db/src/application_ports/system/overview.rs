use std::sync::Arc;

use crate::{ControlDatabaseCluster, OverviewRepository};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::AppResult;

use ryframe_application::ports::system::{
    OverviewPersistencePort, OverviewTrendCount, OverviewTrendSeries, ScheduleOverviewStats,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn OverviewPersistencePort> {
    Arc::new(DatabaseOverviewPersistence { database })
}

struct DatabaseOverviewPersistence {
    database: ControlDatabaseCluster,
}

#[async_trait]
impl OverviewPersistencePort for DatabaseOverviewPersistence {
    async fn database_utc_now(&self) -> AppResult<DateTime<Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn schedule_stats(
        &self,
        tenant_id: &str,
        now: DateTime<Utc>,
    ) -> AppResult<ScheduleOverviewStats> {
        OverviewRepository
            .schedule_stats(self.database.write(), tenant_id, now)
            .await
            .map(|stats| ScheduleOverviewStats {
                enabled: stats.enabled,
                lag_seconds: stats.lag_seconds,
            })
    }

    async fn trends(
        &self,
        tenant_id: &str,
        include_platform: bool,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        bucket_seconds: u32,
    ) -> AppResult<OverviewTrendSeries> {
        let database = self.database.write();
        let (background_jobs, schedules, logins, operations) = tokio::try_join!(
            OverviewRepository.background_job_trends(
                database,
                tenant_id,
                include_platform,
                start,
                end,
                bucket_seconds,
            ),
            OverviewRepository.schedule_execution_trends(
                database,
                tenant_id,
                start,
                end,
                bucket_seconds,
            ),
            OverviewRepository.login_trends(database, tenant_id, start, end, bucket_seconds,),
            OverviewRepository.operation_trends(database, tenant_id, start, end, bucket_seconds,),
        )?;
        Ok(OverviewTrendSeries {
            background_jobs: map_counts(background_jobs),
            schedules: map_counts(schedules),
            logins: map_counts(logins),
            operations: map_counts(operations),
        })
    }
}

fn map_counts(counts: Vec<crate::OverviewTrendCount>) -> Vec<OverviewTrendCount> {
    counts
        .into_iter()
        .map(|count| OverviewTrendCount {
            bucket_index: count.bucket_index,
            dimension: count.dimension,
            count: count.count,
        })
        .collect()
}
