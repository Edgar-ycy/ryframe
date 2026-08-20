use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use serde::Serialize;

use crate::{BackgroundJobQueueStats, JobQueue, OverviewPersistencePort};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverviewRange {
    SixHours,
    TwentyFourHours,
    SevenDays,
}

impl OverviewRange {
    pub fn parse(value: &str) -> AppResult<Self> {
        match value {
            "6h" => Ok(Self::SixHours),
            "24h" => Ok(Self::TwentyFourHours),
            "7d" => Ok(Self::SevenDays),
            _ => Err(AppError::Validation("趋势范围仅支持 6h、24h 或 7d".into())),
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::SixHours => "6h",
            Self::TwentyFourHours => "24h",
            Self::SevenDays => "7d",
        }
    }

    pub const fn bucket_seconds(self) -> u32 {
        match self {
            Self::SixHours => 15 * 60,
            Self::TwentyFourHours => 60 * 60,
            Self::SevenDays => 6 * 60 * 60,
        }
    }

    pub const fn bucket_count(self) -> usize {
        match self {
            Self::SixHours | Self::TwentyFourHours => 24,
            Self::SevenDays => 28,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct OverviewCoreSnapshot {
    pub calculated_at: DateTime<Utc>,
    pub jobs_mode: String,
    pub scheduler_enabled: bool,
    pub background_jobs: BackgroundJobQueueStats,
    pub enabled_schedules: u64,
    pub schedule_lag_seconds: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct OverviewTrendBucket {
    pub started_at: DateTime<Utc>,
    pub background_jobs_created: u64,
    pub schedule_enqueued: u64,
    pub schedule_skipped_misfire: u64,
    pub schedule_skipped_concurrency: u64,
    pub schedule_target_unavailable: u64,
    pub schedule_invalid_configuration: u64,
    pub login_success: u64,
    pub login_failure: u64,
    pub operation_success: u64,
    pub operation_failure: u64,
}

#[derive(Debug, Serialize)]
pub struct OverviewTrends {
    pub calculated_at: DateTime<Utc>,
    pub range: String,
    pub bucket_seconds: u32,
    pub buckets: Vec<OverviewTrendBucket>,
}

pub struct OverviewService {
    persistence: Arc<dyn OverviewPersistencePort>,
    job_queue: Arc<JobQueue>,
    jobs_mode: String,
    scheduler_enabled: bool,
}

impl OverviewService {
    pub fn new(
        persistence: Arc<dyn OverviewPersistencePort>,
        job_queue: Arc<JobQueue>,
        policy: crate::JobRuntimePolicy,
    ) -> Self {
        Self {
            persistence,
            job_queue,
            jobs_mode: policy.worker_mode.as_str().to_owned(),
            scheduler_enabled: policy.scheduler_enabled,
        }
    }

    pub async fn snapshot(&self, actor: &ActorContext) -> AppResult<OverviewCoreSnapshot> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let calculated_at = self
            .persistence
            .database_utc_now()
            .await
            .map_err(main_database_unavailable)?;
        let background_jobs = self
            .job_queue
            .stats_for_tenant(actor)
            .await
            .map_err(main_database_unavailable)?;
        let schedules = self
            .persistence
            .schedule_stats(tenant_id, calculated_at)
            .await
            .map_err(main_database_unavailable)?;
        Ok(OverviewCoreSnapshot {
            calculated_at,
            jobs_mode: self.jobs_mode.clone(),
            scheduler_enabled: self.scheduler_enabled,
            background_jobs,
            enabled_schedules: schedules.enabled,
            schedule_lag_seconds: schedules.lag_seconds,
        })
    }

    pub async fn trends(
        &self,
        actor: &ActorContext,
        range: OverviewRange,
    ) -> AppResult<OverviewTrends> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let calculated_at = self
            .persistence
            .database_utc_now()
            .await
            .map_err(main_database_unavailable)?;
        let bucket_seconds = range.bucket_seconds();
        let bucket_count = range.bucket_count();
        let total_seconds = i64::from(bucket_seconds)
            .checked_mul(
                i64::try_from(bucket_count)
                    .map_err(|_| AppError::Internal("运维趋势时间桶数量无效".into()))?,
            )
            .ok_or_else(|| AppError::Internal("运维趋势时间范围溢出".into()))?;
        let start = calculated_at
            .checked_sub_signed(Duration::seconds(total_seconds))
            .ok_or_else(|| AppError::Internal("运维趋势起始时间溢出".into()))?;
        let include_platform = tenant_id == "system";
        let trends = self
            .persistence
            .trends(
                tenant_id,
                include_platform,
                start,
                calculated_at,
                bucket_seconds,
            )
            .await
            .map_err(main_database_unavailable)?;

        let mut buckets = (0..bucket_count)
            .map(|index| OverviewTrendBucket {
                started_at: start
                    + Duration::seconds(
                        i64::try_from(index).unwrap_or_default() * i64::from(bucket_seconds),
                    ),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        for count in trends.background_jobs {
            if let Some(bucket) = buckets.get_mut(count.bucket_index) {
                bucket.background_jobs_created = count.count;
            }
        }
        for count in trends.schedules {
            let Some(bucket) = buckets.get_mut(count.bucket_index) else {
                continue;
            };
            match count.dimension.as_str() {
                "enqueued" => bucket.schedule_enqueued = count.count,
                "skipped_misfire" => bucket.schedule_skipped_misfire = count.count,
                "skipped_concurrency" => bucket.schedule_skipped_concurrency = count.count,
                "target_unavailable" => bucket.schedule_target_unavailable = count.count,
                "invalid_configuration" => bucket.schedule_invalid_configuration = count.count,
                _ => {}
            }
        }
        for count in trends.logins {
            let Some(bucket) = buckets.get_mut(count.bucket_index) else {
                continue;
            };
            match count.dimension.as_str() {
                "1" => bucket.login_success = count.count,
                "0" => bucket.login_failure = count.count,
                _ => {}
            }
        }
        for count in trends.operations {
            let Some(bucket) = buckets.get_mut(count.bucket_index) else {
                continue;
            };
            match count.dimension.as_str() {
                "1" => bucket.operation_success = count.count,
                "0" => bucket.operation_failure = count.count,
                _ => {}
            }
        }

        Ok(OverviewTrends {
            calculated_at,
            range: range.key().to_owned(),
            bucket_seconds,
            buckets,
        })
    }
}

fn main_database_unavailable(error: AppError) -> AppError {
    match error {
        AppError::Database(_) => AppError::ServiceUnavailable("主数据库暂不可用".into()),
        other => other,
    }
}
