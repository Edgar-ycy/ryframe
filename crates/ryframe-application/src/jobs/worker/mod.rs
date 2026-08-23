use std::{collections::BTreeMap, future::Future, sync::Arc, time::Duration as StdDuration};

use async_trait::async_trait;
use chrono::Duration;
use ryframe_kernel::{AppError, AppResult};
use tokio::{sync::watch, task::JoinHandle, time};
use tracing::Instrument;
use uuid::Uuid;

use super::{
    backoff::{jittered_delay, next_idle_wait},
    queue::JobQueue,
};
use crate::ports::jobs::{
    ClaimedJobRecord, ExecutionTenantScope, FailJobCommand, JobFailureOutcome,
};

/// 任务处理器。实现必须具备幂等性，因为 Worker 提供至少一次投递语义。
///
/// 租约心跳失效时，Worker 会立即丢弃处理器 Future；实现不得把不可取消的业务工作
/// 转移到脱离该 Future 的后台任务中，并应为已经完成的外部副作用提供幂等键或补偿逻辑。
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// 返回唯一的任务类型标识。
    fn job_type(&self) -> &'static str;

    /// 执行已领取任务；返回错误将触发退避重试或死信。
    async fn handle(&self, job: &ClaimedBackgroundJob) -> AppResult<()>;

    /// 判断错误是否属于重试无法恢复的业务错误；默认交给现有重试预算处理。
    fn should_dead_letter(&self, _error: &AppError) -> bool {
        false
    }

    /// 是否提供业务权威状态的丢失任务对账。
    fn has_authoritative_reconciler(&self) -> bool {
        false
    }

    /// 从业务 MySQL 权威表恢复 dead/missing 后台任务；必须多实例幂等。
    async fn reconcile_authoritative_jobs(&self) -> AppResult<()> {
        Ok(())
    }
}

/// 已完成租约领取、可交给业务处理器执行的任务数据。
#[derive(Debug)]
pub struct ClaimedBackgroundJob {
    pub id: i64,
    pub tenant_id: Option<String>,
    pub payload: serde_json::Value,
    pub lease_owner: Option<String>,
    pub attempts: i32,
    pub max_attempts: i32,
}

enum LeaseHeartbeatOutcome<T> {
    Completed(T),
    LeaseLost,
    RenewalFailed(AppError),
}

/// 在处理器运行期间定时续租，并在返回处理结果前再做一次所有权确认。
///
/// 续租失败会直接丢弃处理器 Future，旧 Worker 不再提交成功、重试或死信状态。
async fn run_with_lease_heartbeat<F, R, RFut, T>(
    operation: F,
    heartbeat_interval: StdDuration,
    mut renew: R,
) -> LeaseHeartbeatOutcome<T>
where
    F: Future<Output = T>,
    R: FnMut() -> RFut,
    RFut: Future<Output = AppResult<bool>>,
{
    let first_heartbeat = time::Instant::now() + heartbeat_interval;
    let mut heartbeat = time::interval_at(first_heartbeat, heartbeat_interval);
    tokio::pin!(operation);

    loop {
        tokio::select! {
            biased;
            _ = heartbeat.tick() => {
                match renew().await {
                    Ok(true) => {}
                    Ok(false) => return LeaseHeartbeatOutcome::LeaseLost,
                    Err(error) => return LeaseHeartbeatOutcome::RenewalFailed(error),
                }
            }
            result = &mut operation => {
                return match renew().await {
                    Ok(true) => LeaseHeartbeatOutcome::Completed(result),
                    Ok(false) => LeaseHeartbeatOutcome::LeaseLost,
                    Err(error) => LeaseHeartbeatOutcome::RenewalFailed(error),
                };
            }
        }
    }
}

/// 单次 Worker 循环的结果。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum JobRunResult {
    /// 当前没有可执行任务。
    Idle,
    /// 任务成功完成。
    Succeeded,
    /// 任务被重新安排执行。
    Retried,
    /// 任务进入死信状态。
    Dead,
    /// 处理期间租约已失效，结果不再具有最终性。
    LeaseLost,
}

/// 负责领取、执行和确认任务的 Worker。
#[derive(Clone)]
pub struct JobWorker {
    queue: Arc<JobQueue>,
    execution_tenant_scope: ExecutionTenantScope,
    handlers: Arc<BTreeMap<String, Arc<dyn JobHandler>>>,
    worker_prefix: Arc<str>,
    lease_duration: Duration,
    heartbeat_interval: StdDuration,
    poll_interval: StdDuration,
    max_idle_poll_interval: StdDuration,
    lease_recovery_interval: StdDuration,
    concurrency: usize,
}

mod execution;
mod lifecycle;
mod support;

pub(super) use support::{infrastructure_retry_delay, retry_delay};

impl JobWorker {
    /// 根据运行配置创建 Worker。处理器需要通过 `with_handler` 显式注册。
    pub fn new(
        queue: Arc<JobQueue>,
        policy: &crate::JobWorkerPolicy,
        execution_tenant_scope: ExecutionTenantScope,
    ) -> AppResult<Self> {
        Ok(Self {
            queue,
            execution_tenant_scope,
            handlers: Arc::new(BTreeMap::new()),
            worker_prefix: policy.worker_prefix("ryframe-worker"),
            lease_duration: policy.lease_duration,
            heartbeat_interval: policy.heartbeat_interval,
            poll_interval: policy.poll_interval,
            max_idle_poll_interval: policy.max_idle_poll_interval,
            lease_recovery_interval: policy.lease_recovery_interval,
            concurrency: policy.concurrency,
        })
    }

    /// 注册处理器；重复类型属于启动配置错误。
    pub fn with_handler(mut self, handler: Arc<dyn JobHandler>) -> AppResult<Self> {
        let handlers = Arc::make_mut(&mut self.handlers);
        let job_type = handler.job_type().to_owned();
        if handlers.insert(job_type.clone(), handler).is_some() {
            return Err(AppError::Config(format!(
                "后台任务处理器重复注册: {job_type}"
            )));
        }
        Ok(self)
    }

    /// 判断当前 Worker 是否已经注册指定任务类型的处理器。
    pub fn has_handler(&self, job_type: &str) -> bool {
        self.handlers.contains_key(job_type)
    }
}
