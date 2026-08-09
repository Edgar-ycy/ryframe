use std::{sync::Arc, time::Duration as StdDuration};

/// 后台任务监控的应用层观察者，避免业务层依赖具体指标实现。
pub trait JobMetricsObserver: Send + Sync {
    /// 更新一个已注册任务类型的队列状态计数。
    fn set_queue_depth(&self, job_type: &str, status: &'static str, depth: u64);

    /// 更新一个已注册任务类型中最早可执行任务的等待时长。
    fn set_oldest_ready_age(&self, job_type: &str, age: StdDuration);

    /// 记录一次已经被领取任务的处理时长。
    fn observe_duration(&self, job_type: &str, result: &'static str, duration: StdDuration);

    /// 记录一次持久化队列的领取尝试。
    fn record_claim_attempt(&self, queue: &'static str, result: &'static str);

    /// 记录一次本地或 Redis 唤醒提示的结果。
    fn record_wakeup(&self, queue: &'static str, transport: &'static str, result: &'static str);

    /// 设置当前进程的 Redis 唤醒监听状态。
    fn set_wakeup_listener_up(&self, queue: &'static str, up: bool);

    /// 记录无法识别的 Redis 唤醒协议负载。
    fn record_wakeup_protocol_error(&self, result: &'static str);

    /// 记录一次调度扫描结果。
    fn record_schedule_scan(&self, result: &'static str);

    /// 记录一次计划触发结果。
    fn record_schedule_trigger(&self, outcome: &'static str);

    /// 记录计划时间到扫描领取时间之间的延迟。
    fn observe_schedule_lag(&self, lag: StdDuration);
}

type QueueDepthCallback = dyn Fn(&str, &'static str, u64) + Send + Sync;
type OldestReadyAgeCallback = dyn Fn(&str, StdDuration) + Send + Sync;
type JobDurationCallback = dyn Fn(&str, &'static str, StdDuration) + Send + Sync;
type ClaimAttemptCallback = dyn Fn(&'static str, &'static str) + Send + Sync;
type WakeupCallback = dyn Fn(&'static str, &'static str, &'static str) + Send + Sync;
type WakeupListenerCallback = dyn Fn(&'static str, bool) + Send + Sync;
type WakeupProtocolErrorCallback = dyn Fn(&'static str) + Send + Sync;
type ScheduleScanCallback = dyn Fn(&'static str) + Send + Sync;
type ScheduleTriggerCallback = dyn Fn(&'static str) + Send + Sync;
type ScheduleLagCallback = dyn Fn(StdDuration) + Send + Sync;

/// 使用回调把任务监控事件适配到应用层指标实现。
#[derive(Clone)]
pub struct CallbackJobMetricsObserver {
    on_queue_depth: Arc<QueueDepthCallback>,
    on_oldest_ready_age: Arc<OldestReadyAgeCallback>,
    on_duration: Arc<JobDurationCallback>,
    on_claim_attempt: Arc<ClaimAttemptCallback>,
    on_wakeup: Arc<WakeupCallback>,
    on_wakeup_listener_up: Arc<WakeupListenerCallback>,
    on_wakeup_protocol_error: Arc<WakeupProtocolErrorCallback>,
    on_schedule_scan: Arc<ScheduleScanCallback>,
    on_schedule_trigger: Arc<ScheduleTriggerCallback>,
    on_schedule_lag: Arc<ScheduleLagCallback>,
}

impl CallbackJobMetricsObserver {
    /// 创建由应用层回调驱动的任务监控观察者。
    pub fn new(
        on_queue_depth: Arc<QueueDepthCallback>,
        on_oldest_ready_age: Arc<OldestReadyAgeCallback>,
        on_duration: Arc<JobDurationCallback>,
        on_claim_attempt: Arc<ClaimAttemptCallback>,
        on_wakeup: Arc<WakeupCallback>,
        on_wakeup_listener_up: Arc<WakeupListenerCallback>,
        on_wakeup_protocol_error: Arc<WakeupProtocolErrorCallback>,
    ) -> Self {
        Self {
            on_queue_depth,
            on_oldest_ready_age,
            on_duration,
            on_claim_attempt,
            on_wakeup,
            on_wakeup_listener_up,
            on_wakeup_protocol_error,
            on_schedule_scan: Arc::new(|_| {}),
            on_schedule_trigger: Arc::new(|_| {}),
            on_schedule_lag: Arc::new(|_| {}),
        }
    }

    /// 补充调度扫描、触发和延迟指标回调。
    pub fn with_schedule_callbacks(
        mut self,
        on_schedule_scan: Arc<ScheduleScanCallback>,
        on_schedule_trigger: Arc<ScheduleTriggerCallback>,
        on_schedule_lag: Arc<ScheduleLagCallback>,
    ) -> Self {
        self.on_schedule_scan = on_schedule_scan;
        self.on_schedule_trigger = on_schedule_trigger;
        self.on_schedule_lag = on_schedule_lag;
        self
    }
}

impl JobMetricsObserver for CallbackJobMetricsObserver {
    fn set_queue_depth(&self, job_type: &str, status: &'static str, depth: u64) {
        (self.on_queue_depth)(job_type, status, depth);
    }

    fn set_oldest_ready_age(&self, job_type: &str, age: StdDuration) {
        (self.on_oldest_ready_age)(job_type, age);
    }

    fn observe_duration(&self, job_type: &str, result: &'static str, duration: StdDuration) {
        (self.on_duration)(job_type, result, duration);
    }

    fn record_claim_attempt(&self, queue: &'static str, result: &'static str) {
        (self.on_claim_attempt)(queue, result);
    }

    fn record_wakeup(&self, queue: &'static str, transport: &'static str, result: &'static str) {
        (self.on_wakeup)(queue, transport, result);
    }

    fn set_wakeup_listener_up(&self, queue: &'static str, up: bool) {
        (self.on_wakeup_listener_up)(queue, up);
    }

    fn record_wakeup_protocol_error(&self, result: &'static str) {
        (self.on_wakeup_protocol_error)(result);
    }

    fn record_schedule_scan(&self, result: &'static str) {
        (self.on_schedule_scan)(result);
    }

    fn record_schedule_trigger(&self, outcome: &'static str) {
        (self.on_schedule_trigger)(outcome);
    }

    fn observe_schedule_lag(&self, lag: StdDuration) {
        (self.on_schedule_lag)(lag);
    }
}
