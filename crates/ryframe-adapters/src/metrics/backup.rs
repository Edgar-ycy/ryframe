use chrono::{DateTime, Utc};
use prometheus::{Gauge, GaugeVec, Opts, Registry, core::Collector};
use ryframe_application::ports::backup::BackupHealth;
use std::sync::LazyLock;

struct BackupMetrics {
    collector_up: Gauge,
    collector_last_success: Gauge,
    last_capture: Gauge,
    resources: GaugeVec,
    last_restore: Gauge,
    restore_succeeded: Gauge,
    restore_duration: Gauge,
    recovery_point_age: Gauge,
    restore_runs: GaugeVec,
}

fn gauge(name: &str, help: &str) -> Gauge {
    Gauge::new(name, help).expect("备份指标定义有效")
}

static METRICS: LazyLock<BackupMetrics> = LazyLock::new(|| BackupMetrics {
    collector_up: gauge("backup_collector_up", "备份状态是否成功从控制库读取"),
    collector_last_success: gauge(
        "backup_collector_last_success_timestamp_seconds",
        "备份状态采集器最后一次成功完成采集的时间",
    ),
    last_capture: gauge(
        "backup_last_success_timestamp_seconds",
        "必需目标中最旧有效备份的实际采集时间",
    ),
    resources: GaugeVec::new(
        Opts::new("backup_resources", "必需备份资源的固定分类汇总"),
        &["state"],
    )
    .expect("备份资源指标定义有效"),
    last_restore: gauge(
        "restore_last_completed_timestamp_seconds",
        "最近一次恢复演练的完成时间",
    ),
    restore_succeeded: gauge("restore_last_succeeded", "最近一次恢复演练是否成功"),
    restore_duration: gauge(
        "restore_duration_seconds",
        "最近一次演练从外部恢复开始到业务验证完成的耗时",
    ),
    recovery_point_age: gauge(
        "restore_recovery_point_age_seconds",
        "最近一次演练恢复点距离故障的秒数",
    ),
    restore_runs: GaugeVec::new(
        Opts::new("restore_runs", "运行中与超时恢复演练数量"),
        &["state"],
    )
    .expect("恢复演练指标定义有效"),
});

pub(super) fn register(registry: &Registry) {
    for collector in [
        Box::new(METRICS.collector_up.clone()) as Box<dyn Collector>,
        Box::new(METRICS.collector_last_success.clone()),
        Box::new(METRICS.last_capture.clone()),
        Box::new(METRICS.resources.clone()),
        Box::new(METRICS.last_restore.clone()),
        Box::new(METRICS.restore_succeeded.clone()),
        Box::new(METRICS.restore_duration.clone()),
        Box::new(METRICS.recovery_point_age.clone()),
        Box::new(METRICS.restore_runs.clone()),
    ] {
        registry.register(collector).expect("注册备份状态指标");
    }
}

pub fn set_backup_health(health: &BackupHealth, collected_at: DateTime<Utc>) {
    super::ensure_registered();
    METRICS.collector_up.set(1.0);
    METRICS
        .collector_last_success
        .set(collected_at.timestamp() as f64);
    METRICS.last_capture.set(
        health
            .oldest_capture
            .map_or(0.0, |time| time.timestamp() as f64),
    );
    for (state, value) in [
        ("required", health.required_resources),
        ("missing", health.missing_resources),
        ("expired", health.expired_resources),
        ("invalid", health.invalid_resources),
    ] {
        METRICS
            .resources
            .with_label_values(&[state])
            .set(value as f64);
    }
    METRICS.last_restore.set(
        health
            .last_restore_completed
            .map_or(0.0, |time| time.timestamp() as f64),
    );
    METRICS
        .restore_succeeded
        .set(f64::from(health.last_restore_succeeded));
    METRICS
        .restore_duration
        .set(health.restore_duration_seconds.unwrap_or(0) as f64);
    METRICS
        .recovery_point_age
        .set(health.recovery_point_age_seconds.unwrap_or(0) as f64);
    for (state, value) in [
        ("running", health.restore_running),
        ("overdue", health.restore_overdue),
    ] {
        METRICS
            .restore_runs
            .with_label_values(&[state])
            .set(value as f64);
    }
}

pub fn set_backup_collector_failed() {
    super::ensure_registered();
    METRICS.collector_up.set(0.0);
}
