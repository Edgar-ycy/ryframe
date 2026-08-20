use chrono::{DateTime, Utc};

use crate::PersistenceFuture;

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

pub trait OverviewPersistencePort: Send + Sync {
    fn database_utc_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn schedule_stats<'a>(
        &'a self,
        tenant_id: &'a str,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, ScheduleOverviewStats>;

    fn trends<'a>(
        &'a self,
        tenant_id: &'a str,
        include_platform: bool,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        bucket_seconds: u32,
    ) -> PersistenceFuture<'a, OverviewTrendSeries>;
}
