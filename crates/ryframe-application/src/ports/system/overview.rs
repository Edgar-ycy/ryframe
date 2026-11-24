use async_trait::async_trait;
use ryframe_kernel::AppResult;
use time::OffsetDateTime;

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
    async fn database_utc_now(&self) -> AppResult<OffsetDateTime>;

    async fn schedule_stats(
        &self,
        tenant_id: &str,
        now: OffsetDateTime,
    ) -> AppResult<ScheduleOverviewStats>;

    async fn trends(
        &self,
        tenant_id: &str,
        include_platform: bool,
        start: OffsetDateTime,
        end: OffsetDateTime,
        bucket_seconds: u32,
    ) -> AppResult<OverviewTrendSeries>;
}
