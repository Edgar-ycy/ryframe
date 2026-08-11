use std::{sync::Arc, time::Duration as StdDuration};

/// Cron 调度监控观察者，与通用后台任务指标保持单向隔离。
pub trait ScheduleMetricsObserver: Send + Sync {
    /// 记录一次调度扫描结果。
    fn record_scan(&self, result: &'static str);

    /// 记录一次已经提交执行历史的计划触发结果。
    fn record_trigger(&self, outcome: &'static str);

    /// 记录计划时间到扫描领取时间之间的延迟。
    fn observe_lag(&self, lag: StdDuration);
}

type ScheduleScanCallback = dyn Fn(&'static str) + Send + Sync;
type ScheduleTriggerCallback = dyn Fn(&'static str) + Send + Sync;
type ScheduleLagCallback = dyn Fn(StdDuration) + Send + Sync;

/// 使用回调把 Cron 调度事件适配到应用层指标实现。
pub struct CallbackScheduleMetricsObserver {
    on_scan: Arc<ScheduleScanCallback>,
    on_trigger: Arc<ScheduleTriggerCallback>,
    on_lag: Arc<ScheduleLagCallback>,
}

impl CallbackScheduleMetricsObserver {
    pub fn new(
        on_scan: Arc<ScheduleScanCallback>,
        on_trigger: Arc<ScheduleTriggerCallback>,
        on_lag: Arc<ScheduleLagCallback>,
    ) -> Self {
        Self {
            on_scan,
            on_trigger,
            on_lag,
        }
    }
}

impl ScheduleMetricsObserver for CallbackScheduleMetricsObserver {
    fn record_scan(&self, result: &'static str) {
        (self.on_scan)(result);
    }

    fn record_trigger(&self, outcome: &'static str) {
        (self.on_trigger)(outcome);
    }

    fn observe_lag(&self, lag: StdDuration) {
        (self.on_lag)(lag);
    }
}
