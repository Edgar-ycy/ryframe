use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use time::OffsetDateTime;

/// 部署级备份与恢复汇总，不包含租户、端点或对象键维度。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct BackupHealth {
    pub required_resources: u64,
    pub missing_resources: u64,
    pub expired_resources: u64,
    pub invalid_resources: u64,
    #[serde(default, with = "optional_utc_timestamp")]
    pub oldest_capture: Option<OffsetDateTime>,
    #[serde(default, with = "optional_utc_timestamp")]
    pub last_restore_completed: Option<OffsetDateTime>,
    pub last_restore_succeeded: bool,
    pub restore_duration_seconds: Option<i64>,
    pub recovery_point_age_seconds: Option<i64>,
    pub restore_running: u64,
    pub restore_overdue: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupCollectionStatus {
    Unknown,
    Available,
    Unavailable,
    Stale,
}

impl BackupCollectionStatus {
    pub const fn available(self) -> bool {
        matches!(self, Self::Available)
    }
}

#[derive(Clone, Debug)]
pub struct BackupHealthSnapshot {
    pub status: BackupCollectionStatus,
    pub last_attempt_at: Option<OffsetDateTime>,
    pub last_success_at: Option<OffsetDateTime>,
    /// 最近一次有效汇总。状态不可用或陈旧时仅供诊断，不能视为当前状态。
    pub last_valid_health: Option<BackupHealth>,
}

impl BackupHealthSnapshot {
    pub const fn available(&self) -> bool {
        self.status.available()
    }
}

#[derive(Clone, Debug)]
struct Observation {
    status: BackupCollectionStatus,
    last_attempt_at: Option<OffsetDateTime>,
    last_success_at: Option<OffsetDateTime>,
    last_valid_health: Option<BackupHealth>,
    observed_at: Option<Instant>,
}

/// 后台采集写入、运行时状态只读的备份健康缓存。
#[derive(Clone)]
pub struct BackupHealthCache {
    inner: Arc<RwLock<Observation>>,
    max_age: Duration,
    clock: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl BackupHealthCache {
    pub fn new(max_age: Duration) -> Self {
        Self::with_clock(max_age, Instant::now)
    }

    /// 注入单调时钟用于精确验证过期边界。
    pub fn with_clock(
        max_age: Duration,
        clock: impl Fn() -> Instant + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Observation {
                status: BackupCollectionStatus::Unknown,
                last_attempt_at: None,
                last_success_at: None,
                last_valid_health: None,
                observed_at: None,
            })),
            max_age,
            clock: Arc::new(clock),
        }
    }

    pub fn update_success(&self, health: BackupHealth, collected_at: OffsetDateTime) {
        self.write(|current| {
            current.status = BackupCollectionStatus::Available;
            current.last_attempt_at = Some(collected_at);
            current.last_success_at = Some(collected_at);
            current.last_valid_health = Some(health);
            current.observed_at = Some((self.clock)());
        });
    }

    pub fn update_failure(&self, attempted_at: OffsetDateTime) {
        self.write(|current| {
            current.status = BackupCollectionStatus::Unavailable;
            current.last_attempt_at = Some(attempted_at);
            current.observed_at = Some((self.clock)());
        });
    }

    pub fn snapshot(&self) -> BackupHealthSnapshot {
        let current = match self.inner.read() {
            Ok(current) => current.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let stale = current.observed_at.is_some_and(|observed_at| {
            (self.clock)().saturating_duration_since(observed_at) > self.max_age
        });
        BackupHealthSnapshot {
            status: if stale {
                BackupCollectionStatus::Stale
            } else {
                current.status
            },
            last_attempt_at: current.last_attempt_at,
            last_success_at: current.last_success_at,
            last_valid_health: current.last_valid_health,
        }
    }

    fn write(&self, update: impl FnOnce(&mut Observation)) {
        match self.inner.write() {
            Ok(mut current) => update(&mut current),
            Err(poisoned) => update(&mut poisoned.into_inner()),
        }
    }
}

/// 备份健康汇总与监控响应共用的 UTC 字符串格式。
pub mod optional_utc_timestamp {
    use std::fmt::Write as _;

    use serde::{Deserialize, Deserializer, Serializer, de::Error as _, ser::Error as _};
    use time::{
        OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339,
        macros::format_description,
    };

    pub fn serialize<S: Serializer>(
        value: &Option<OffsetDateTime>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let Some(value) = value else {
            return serializer.serialize_none();
        };
        let utc = value
            .checked_to_offset(UtcOffset::UTC)
            .ok_or_else(|| S::Error::custom("备份健康时间超出 UTC 表示范围"))?;
        let mut encoded = utc
            .format(format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second]"
            ))
            .map_err(S::Error::custom)?;
        let nanos = utc.nanosecond();
        if nanos != 0 {
            let (fraction, digits) = if nanos.is_multiple_of(1_000_000) {
                (nanos / 1_000_000, 3)
            } else if nanos.is_multiple_of(1_000) {
                (nanos / 1_000, 6)
            } else {
                (nanos, 9)
            };
            write!(encoded, ".{fraction:0digits$}").map_err(S::Error::custom)?;
        }
        encoded.push('Z');
        serializer.serialize_some(&encoded)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<OffsetDateTime>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|value| {
                if value.get(17..19) == Some("60") {
                    return Err(D::Error::custom("备份健康时间不能精确表示闰秒"));
                }
                OffsetDateTime::parse(&value, &Rfc3339)
                    .map_err(D::Error::custom)?
                    .checked_to_offset(UtcOffset::UTC)
                    .ok_or_else(|| D::Error::custom("备份健康时间超出 UTC 表示范围"))
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn health_timestamps_keep_existing_json_fraction_and_missing_field_support() {
        let health = BackupHealth {
            oldest_capture: Some(
                OffsetDateTime::from_unix_timestamp_nanos(1_700_000_000_123_400_000).unwrap(),
            ),
            ..Default::default()
        };
        let mut encoded = serde_json::to_value(&health).unwrap();
        assert_eq!(encoded["oldest_capture"], "2023-11-14T22:13:20.123400Z");
        assert!(encoded["last_restore_completed"].is_null());
        let decoded: BackupHealth = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded.oldest_capture, health.oldest_capture);
        encoded["oldest_capture"] = serde_json::json!("2023-11-14T23:13:20.123400+01:00");
        encoded
            .as_object_mut()
            .unwrap()
            .remove("last_restore_completed");
        let decoded: BackupHealth = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded.oldest_capture, health.oldest_capture);
        assert!(decoded.last_restore_completed.is_none());

        encoded["oldest_capture"] = serde_json::json!("2016-12-31T23:59:60Z");
        assert!(serde_json::from_value::<BackupHealth>(encoded).is_err());
    }

    #[test]
    fn failures_and_staleness_never_publish_cached_health_as_available() {
        let start = Instant::now();
        let now = Arc::new(Mutex::new(start));
        let clock = Arc::clone(&now);
        let cache =
            BackupHealthCache::with_clock(Duration::from_secs(150), move || *clock.lock().unwrap());
        let collected = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        assert_eq!(cache.snapshot().status, BackupCollectionStatus::Unknown);
        cache.update_success(
            BackupHealth {
                required_resources: 5,
                ..Default::default()
            },
            collected,
        );
        assert!(cache.snapshot().available());
        cache.update_failure(collected + time::Duration::seconds(60));
        let failed = cache.snapshot();
        assert_eq!(failed.status, BackupCollectionStatus::Unavailable);
        assert!(!failed.available());
        assert_eq!(failed.last_success_at, Some(collected));
        assert_eq!(failed.last_valid_health.unwrap().required_resources, 5);
        *now.lock().unwrap() = start + Duration::from_secs(211);
        let stale = cache.snapshot();
        assert_eq!(stale.status, BackupCollectionStatus::Stale);
        assert!(!stale.available());
        assert_eq!(stale.last_success_at, Some(collected));
    }

    #[test]
    fn clone_observes_atomic_updates_and_exact_expiry_boundary() {
        let start = Instant::now();
        let now = Arc::new(Mutex::new(start));
        let clock = Arc::clone(&now);
        let cache =
            BackupHealthCache::with_clock(Duration::from_secs(10), move || *clock.lock().unwrap());
        let reader = cache.clone();
        let collected = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        cache.update_success(BackupHealth::default(), collected);
        *now.lock().unwrap() = start + Duration::from_secs(10);
        assert_eq!(reader.snapshot().status, BackupCollectionStatus::Available);
        *now.lock().unwrap() = start + Duration::from_secs(10) + Duration::from_nanos(1);
        assert_eq!(reader.snapshot().status, BackupCollectionStatus::Stale);
    }
}
