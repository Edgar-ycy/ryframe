use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::entities::background_job;

/// 用于写入持久化异步任务的输入。
///
/// `dedupe_key` 按 `job_type` 隔离；提供该值后，首次调用创建任务，后续调用返回同一任务。
#[derive(Clone, Debug)]
pub struct EnqueueBackgroundJob {
    pub tenant_id: Option<String>,
    pub schedule_id: Option<i64>,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub max_runtime_seconds: Option<i32>,
    pub job_type: String,
    pub payload: Value,
    pub priority: i32,
    pub available_at: DateTime<Utc>,
    pub max_attempts: i32,
    pub dedupe_key: Option<String>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

/// 幂等入队操作的结果。
#[derive(Clone, Debug)]
pub struct EnqueueBackgroundJobResult {
    pub job: background_job::Model,
    /// `false` 表示其他请求已创建同一 `(job_type, dedupe_key)` 任务。
    pub inserted: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExpiredLeaseRecovery {
    pub requeued: u64,
    pub dead: u64,
}

#[derive(Clone, Debug, Default)]
pub struct BackgroundJobFilter<'a> {
    pub tenant_id: Option<&'a str>,
    pub include_platform: bool,
    pub schedule_id: Option<i64>,
    pub job_type: Option<&'a str>,
    pub status: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackgroundJobStats {
    pub total: u64,
    pub pending: u64,
    pub running: u64,
    pub succeeded: u64,
    pub dead: u64,
    pub ready: u64,
}

/// 单个已注册任务类型的低基数队列指标。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackgroundJobTypeStats {
    pub job_type: String,
    pub pending: u64,
    pub running: u64,
    pub dead: u64,
    pub ready: u64,
    pub oldest_ready_age: Option<StdDuration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobFailureDisposition {
    /// 当前持有的租约已完成，任务会在指定时间重新变为可执行。
    Retried { available_at: DateTime<Utc> },
    /// 已耗尽最大领取次数。
    Dead,
    /// 其他 Worker 持有该租约，或该租约已过期并被重新领取。
    LeaseLost,
}

/// 仅适用于 MySQL 的持久化任务队列仓储。
pub struct BackgroundJobRepository;
