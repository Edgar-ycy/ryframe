use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

/// 就绪检查中单个依赖的最近状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependencyStatus {
    Unknown,
    Up,
    Down,
    OptionalDegraded,
    NotRequired,
}

impl DependencyStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Up => "up",
            Self::Down => "down",
            Self::OptionalDegraded => "optional_degraded",
            Self::NotRequired => "not_required",
        }
    }

    const fn blocks_readiness(self) -> bool {
        matches!(self, Self::Unknown | Self::Down)
    }
}

/// 一次只读的依赖健康快照。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DependencyHealthSnapshot {
    pub mysql: DependencyStatus,
    pub redis: DependencyStatus,
    pub object_storage: DependencyStatus,
    pub stale: bool,
}

impl DependencyHealthSnapshot {
    pub const fn is_ready(self) -> bool {
        !self.stale
            && !self.mysql.blocks_readiness()
            && !self.redis.blocks_readiness()
            && !self.object_storage.blocks_readiness()
    }
}

#[derive(Clone, Copy, Debug)]
struct DependencyHealthObservation {
    mysql: DependencyStatus,
    redis: DependencyStatus,
    object_storage: DependencyStatus,
    observed_at: Option<Instant>,
}

/// 由后台探测任务写入、由就绪端点只读的依赖健康缓存。
#[derive(Clone, Debug)]
pub struct DependencyHealthCache {
    inner: Arc<RwLock<DependencyHealthObservation>>,
    redis_required: bool,
    object_storage_required: bool,
    max_age: Duration,
}

impl DependencyHealthCache {
    pub fn new(redis_required: bool, object_storage_required: bool, max_age: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Self::initial_observation(
                redis_required,
                object_storage_required,
            ))),
            redis_required,
            object_storage_required,
            max_age,
        }
    }

    /// 原子替换一轮后台探测结果。
    pub fn update(&self, mysql_ok: bool, redis_reachable: bool, object_storage_ok: bool) {
        let observation = DependencyHealthObservation {
            mysql: status(mysql_ok),
            redis: if redis_reachable {
                DependencyStatus::Up
            } else if self.redis_required {
                DependencyStatus::Down
            } else {
                DependencyStatus::OptionalDegraded
            },
            object_storage: if self.object_storage_required {
                status(object_storage_ok)
            } else {
                DependencyStatus::NotRequired
            },
            observed_at: Some(Instant::now()),
        };

        match self.inner.write() {
            Ok(mut current) => *current = observation,
            Err(poisoned) => *poisoned.into_inner() = observation,
        }
    }

    pub const fn redis_required(&self) -> bool {
        self.redis_required
    }

    pub const fn object_storage_required(&self) -> bool {
        self.object_storage_required
    }

    /// 返回最近一次未过期结果；探测任务失联时按未知状态 fail-closed。
    pub fn snapshot(&self) -> DependencyHealthSnapshot {
        let observation = match self.inner.read() {
            Ok(current) => *current,
            Err(poisoned) => *poisoned.into_inner(),
        };
        let stale = observation
            .observed_at
            .is_none_or(|observed_at| observed_at.elapsed() > self.max_age);
        let effective = if stale {
            Self::initial_observation(self.redis_required, self.object_storage_required)
        } else {
            observation
        };

        DependencyHealthSnapshot {
            mysql: effective.mysql,
            redis: effective.redis,
            object_storage: effective.object_storage,
            stale,
        }
    }

    fn initial_observation(
        redis_required: bool,
        object_storage_required: bool,
    ) -> DependencyHealthObservation {
        DependencyHealthObservation {
            mysql: DependencyStatus::Unknown,
            redis: if redis_required {
                DependencyStatus::Unknown
            } else {
                DependencyStatus::OptionalDegraded
            },
            object_storage: if object_storage_required {
                DependencyStatus::Unknown
            } else {
                DependencyStatus::NotRequired
            },
            observed_at: None,
        }
    }
}

const fn status(healthy: bool) -> DependencyStatus {
    if healthy {
        DependencyStatus::Up
    } else {
        DependencyStatus::Down
    }
}

#[cfg(test)]
mod tests {
    use super::{DependencyHealthCache, DependencyStatus};
    use std::time::{Duration, Instant};

    #[test]
    fn required_dependencies_fail_closed_until_first_observation() {
        let cache = DependencyHealthCache::new(true, true, Duration::from_secs(15));

        let snapshot = cache.snapshot();
        assert!(!snapshot.is_ready());
        assert!(snapshot.stale);
        assert_eq!(snapshot.mysql, DependencyStatus::Unknown);
        assert_eq!(snapshot.redis, DependencyStatus::Unknown);
        assert_eq!(snapshot.object_storage, DependencyStatus::Unknown);
    }

    #[test]
    fn optional_redis_degrades_without_blocking_readiness() {
        let cache = DependencyHealthCache::new(false, true, Duration::from_secs(15));
        cache.update(true, false, true);

        let snapshot = cache.snapshot();
        assert!(snapshot.is_ready());
        assert_eq!(snapshot.redis, DependencyStatus::OptionalDegraded);
    }

    #[test]
    fn failed_required_dependency_blocks_readiness() {
        let cache = DependencyHealthCache::new(true, true, Duration::from_secs(15));
        cache.update(true, false, true);

        let snapshot = cache.snapshot();
        assert!(!snapshot.is_ready());
        assert_eq!(snapshot.redis, DependencyStatus::Down);
    }

    #[test]
    fn unneeded_object_storage_does_not_block_worker_readiness() {
        let cache = DependencyHealthCache::new(true, false, Duration::from_secs(15));
        cache.update(true, true, false);

        let snapshot = cache.snapshot();
        assert!(snapshot.is_ready());
        assert_eq!(snapshot.object_storage, DependencyStatus::NotRequired);
    }

    #[test]
    fn expired_observation_returns_to_unknown_state() {
        let cache = DependencyHealthCache::new(true, true, Duration::from_secs(15));
        cache.update(true, true, true);
        match cache.inner.write() {
            Ok(mut observation) => {
                observation.observed_at = Some(Instant::now() - Duration::from_secs(16));
            }
            Err(poisoned) => {
                poisoned.into_inner().observed_at = Some(Instant::now() - Duration::from_secs(16));
            }
        }

        let snapshot = cache.snapshot();
        assert!(!snapshot.is_ready());
        assert!(snapshot.stale);
        assert_eq!(snapshot.mysql, DependencyStatus::Unknown);
    }
}
