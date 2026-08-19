use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 按两倍增长计算下一次空闲轮询间隔，并限制在配置边界内。
pub(super) fn next_idle_wait(
    current: Duration,
    min_interval: Duration,
    max_interval: Duration,
) -> Duration {
    if current >= max_interval {
        return max_interval;
    }
    std::cmp::min(
        max_interval,
        std::cmp::max(min_interval, current.saturating_mul(2)),
    )
}

/// 在基础等待时间上加入正负百分之二十的抖动，分散并发轮询。
pub(super) fn jittered_delay(base: Duration) -> Duration {
    let base_ms = base.as_millis().max(1) as i64;
    let jitter = base_ms / 5;
    if jitter == 0 {
        return base;
    }
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| i64::from(time.subsec_nanos()))
        .unwrap_or(0);
    let offset = (seed.rem_euclid(2 * jitter + 1)) - jitter;
    let actual = base_ms.saturating_add(offset).max(1);
    Duration::from_millis(actual as u64)
}
