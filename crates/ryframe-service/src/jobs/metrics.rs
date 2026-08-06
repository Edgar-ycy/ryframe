use std::{sync::Arc, time::Duration as StdDuration};

/// 后台任务监控的应用层观察者，避免业务层依赖具体指标实现。
pub trait JobMetricsObserver: Send + Sync {
    /// 更新一个已注册任务类型的队列状态计数。
    fn set_queue_depth(&self, job_type: &str, status: &'static str, depth: u64);

    /// 更新一个已注册任务类型中最早可执行任务的等待时长。
    fn set_oldest_ready_age(&self, job_type: &str, age: StdDuration);

    /// 记录一次已经被领取任务的处理时长。
    fn observe_duration(&self, job_type: &str, result: &'static str, duration: StdDuration);
}

type QueueDepthCallback = dyn Fn(&str, &'static str, u64) + Send + Sync;
type OldestReadyAgeCallback = dyn Fn(&str, StdDuration) + Send + Sync;
type JobDurationCallback = dyn Fn(&str, &'static str, StdDuration) + Send + Sync;

/// 使用回调把任务监控事件适配到应用层指标实现。
#[derive(Clone)]
pub struct CallbackJobMetricsObserver {
    on_queue_depth: Arc<QueueDepthCallback>,
    on_oldest_ready_age: Arc<OldestReadyAgeCallback>,
    on_duration: Arc<JobDurationCallback>,
}

impl CallbackJobMetricsObserver {
    /// 创建由应用层回调驱动的任务监控观察者。
    pub fn new(
        on_queue_depth: Arc<QueueDepthCallback>,
        on_oldest_ready_age: Arc<OldestReadyAgeCallback>,
        on_duration: Arc<JobDurationCallback>,
    ) -> Self {
        Self {
            on_queue_depth,
            on_oldest_ready_age,
            on_duration,
        }
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
}
