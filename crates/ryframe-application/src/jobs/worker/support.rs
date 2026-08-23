use super::*;

/// 生成指数退避等待时间，最高五分钟。
pub(in super::super) fn retry_delay(attempts: i32) -> Duration {
    let exponent = attempts.saturating_sub(1).clamp(0, 6) as u32;
    let seconds = 5_i64.saturating_mul(1_i64 << exponent).min(300);
    Duration::seconds(seconds)
}

/// 计算基础设施调用连续失败后的本地退避时间，避免数据库故障时形成高频请求和日志。
pub(in super::super) fn infrastructure_retry_delay(
    poll_interval: StdDuration,
    consecutive_failures: u32,
) -> StdDuration {
    const MAX_DELAY: StdDuration = StdDuration::from_secs(30);
    let exponent = consecutive_failures.saturating_sub(1).min(30);
    let multiplier = 1_u32 << exponent;
    poll_interval.saturating_mul(multiplier).min(MAX_DELAY)
}
