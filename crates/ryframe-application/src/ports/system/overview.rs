use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::AppResult;

#[derive(Clone, Debug)]
pub struct OverviewTrendCount {
    pub bucket_index: usize,
    pub dimension: String,
    pub count: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScheduleOverviewStats {
    pub enabled: u64,
    pub lag_seconds: f64,
}

#[derive(Debug, Default)]
pub struct OverviewTrendSeries {
    pub background_jobs: Vec<OverviewTrendCount>,
    pub schedules: Vec<OverviewTrendCount>,
    pub logins: Vec<OverviewTrendCount>,
    pub operations: Vec<OverviewTrendCount>,
}

#[async_trait]
pub trait OverviewPersistencePort: Send + Sync {
    async fn database_utc_now(&self) -> AppResult<DateTime<Utc>>;

    async fn schedule_stats(
        &self,
        tenant_id: &str,
        now: DateTime<Utc>,
    ) -> AppResult<ScheduleOverviewStats>;

    async fn trends(
        &self,
        tenant_id: &str,
        include_platform: bool,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        bucket_seconds: u32,
    ) -> AppResult<OverviewTrendSeries>;
}
