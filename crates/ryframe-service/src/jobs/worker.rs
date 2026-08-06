use std::{collections::BTreeMap, future::Future, sync::Arc, time::Duration as StdDuration};

use async_trait::async_trait;
use chrono::Duration;
use ryframe_config::JobConfig;
use ryframe_db::{JobFailureDisposition, background_job};
use ryframe_kernel::{AppError, AppResult};
use tokio::{sync::watch, task::JoinHandle, time};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

use super::queue::JobQueue;

/// 任务处理器。实现必须具备幂等性，因为 Worker 提供至少一次投递语义。
///
/// 租约心跳失效时，Worker 会立即丢弃处理器 Future；实现不得把不可取消的业务工作
/// 转移到脱离该 Future 的后台任务中，并应为已经完成的外部副作用提供幂等键或补偿逻辑。
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// 返回唯一的任务类型标识。
    fn job_type(&self) -> &'static str;

    /// 执行已领取任务；返回错误将触发退避重试或死信。
    async fn handle(&self, job: &background_job::Model) -> AppResult<()>;
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
    handlers: Arc<BTreeMap<String, Arc<dyn JobHandler>>>,
    worker_prefix: String,
    lease_duration: Duration,
    heartbeat_interval: StdDuration,
    poll_interval: StdDuration,
    concurrency: usize,
}

impl JobWorker {
    /// 根据运行配置创建 Worker。处理器需要通过 `with_handler` 显式注册。
    pub fn new(queue: Arc<JobQueue>, config: &JobConfig) -> AppResult<Self> {
        let lease_seconds = i64::try_from(config.lease_seconds)
            .map_err(|_| AppError::Config("jobs.lease_seconds 超出支持范围".into()))?;
        let worker_prefix = config
            .worker_id
            .clone()
            .unwrap_or_else(|| "ryframe-worker".into());
        Ok(Self {
            queue,
            handlers: Arc::new(BTreeMap::new()),
            worker_prefix,
            lease_duration: Duration::seconds(lease_seconds),
            heartbeat_interval: StdDuration::from_secs(config.heartbeat_seconds),
            poll_interval: StdDuration::from_millis(config.poll_interval_ms),
            concurrency: config.concurrency,
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

    /// 启动配置数量的并行消费循环，并在收到关闭信号后有序退出。
    pub fn spawn(self, shutdown: watch::Receiver<bool>) -> Vec<JoinHandle<()>> {
        let instance = Uuid::new_v4().simple().to_string();
        let mut tasks = (0..self.concurrency)
            .map(|slot| {
                let worker = self.clone();
                let worker_id = format!("{}-{slot}-{}", worker.worker_prefix, &instance[..12]);
                let receiver = shutdown.clone();
                tokio::spawn(async move {
                    worker.run_until_shutdown(worker_id, receiver).await;
                })
            })
            .collect::<Vec<_>>();

        if self.queue.has_metrics_observer() && !self.handlers.is_empty() {
            let queue = self.queue.clone();
            let job_types = self.handlers.keys().cloned().collect::<Vec<_>>();
            let mut receiver = shutdown.clone();
            tasks.push(tokio::spawn(async move {
                let mut collection_degraded = false;
                loop {
                    match queue.report_metrics_for_types(&job_types).await {
                        Ok(()) if collection_degraded => {
                            tracing::info!("后台任务队列指标采集已恢复");
                            collection_degraded = false;
                        }
                        Ok(()) => {}
                        Err(error) if collection_degraded => {
                            tracing::debug!(%error, "后台任务队列指标采集仍不可用");
                        }
                        Err(error) => {
                            tracing::warn!(%error, "后台任务队列指标采集失败");
                            collection_degraded = true;
                        }
                    }
                    tokio::select! {
                        _ = time::sleep(StdDuration::from_secs(30)) => {}
                        changed = receiver.changed() => {
                            if changed.is_err() || *receiver.borrow() {
                                break;
                            }
                        }
                    }
                }
            }));
        }

        let worker = self.clone();
        tasks.push(tokio::spawn(async move {
            worker.recover_expired_leases_until_shutdown(shutdown).await;
        }));

        tasks
    }

    /// 执行一次领取和处理，供单次执行模式及自定义运行器使用。
    pub async fn run_once(&self, worker_id: &str) -> AppResult<JobRunResult> {
        let now = self.queue.database_now().await?;
        let Some(job) = self
            .queue
            .repository()
            .claim_next(self.queue.primary(), worker_id, self.lease_duration, now)
            .await?
        else {
            return Ok(JobRunResult::Idle);
        };

        let job_type = job.job_type.clone();
        let metric_job_type =
            bounded_job_type_label(self.handlers.contains_key(&job_type), &job_type);
        let started = std::time::Instant::now();
        let result = self.run_claimed_job(job, worker_id).await;
        self.queue.observe_job_duration(
            metric_job_type,
            job_run_result_label(&result),
            started.elapsed(),
        );
        result
    }

    /// 处理已完成租约领取的任务，并保留原有状态转换语义。
    async fn run_claimed_job(
        &self,
        job: background_job::Model,
        worker_id: &str,
    ) -> AppResult<JobRunResult> {
        let Some(handler) = self.handlers.get(&job.job_type).cloned() else {
            let now = self.queue.database_now().await?;
            let failure_reason = format!("未注册任务处理器: {}", job.job_type);
            let completed = self
                .queue
                .repository()
                .dead_letter(
                    self.queue.primary(),
                    job.id,
                    worker_id,
                    &failure_reason,
                    now,
                )
                .await?;
            return Ok(if completed {
                tracing::error!(
                    job_id = job.id,
                    job_type = %job.job_type,
                    worker_id,
                    attempts = job.attempts,
                    max_attempts = job.max_attempts,
                    failure_reason,
                    "后台任务因未注册处理器进入死信状态"
                );
                JobRunResult::Dead
            } else {
                JobRunResult::LeaseLost
            });
        };

        let span = tracing::info_span!("background_job", job_type = %job.job_type);
        let _ = span.set_parent(crate::trace_context::extract_parent_context(
            job.traceparent.as_deref(),
            job.tracestate.as_deref(),
        ));
        async {
            let heartbeat_queue = self.queue.clone();
            let heartbeat_worker_id = worker_id.to_owned();
            let heartbeat_job_id = job.id;
            let lease_duration = self.lease_duration;
            let handler_result = match run_with_lease_heartbeat(
                handler.handle(&job),
                self.heartbeat_interval,
                move || {
                    let queue = heartbeat_queue.clone();
                    let worker_id = heartbeat_worker_id.clone();
                    async move {
                        let now = queue.database_now().await?;
                        queue
                            .repository()
                            .renew_lease(
                                queue.primary(),
                                heartbeat_job_id,
                                &worker_id,
                                lease_duration,
                                now,
                            )
                            .await
                    }
                },
            )
            .await
            {
                LeaseHeartbeatOutcome::Completed(result) => result,
                LeaseHeartbeatOutcome::LeaseLost => {
                    tracing::warn!(
                        job_id = job.id,
                        worker_id,
                        "后台任务租约已失效，处理器已取消且不会提交最终状态"
                    );
                    return Ok(JobRunResult::LeaseLost);
                }
                LeaseHeartbeatOutcome::RenewalFailed(error) => {
                    tracing::warn!(
                        %error,
                        job_id = job.id,
                        worker_id,
                        "后台任务续租失败，处理器已取消且不会提交最终状态"
                    );
                    return Ok(JobRunResult::LeaseLost);
                }
            };

            match handler_result {
                Ok(()) => {
                    let now = self.queue.database_now().await?;
                    let completed = self
                        .queue
                        .repository()
                        .complete(self.queue.primary(), job.id, worker_id, now)
                        .await?;
                    Ok(if completed {
                        JobRunResult::Succeeded
                    } else {
                        JobRunResult::LeaseLost
                    })
                }
                Err(error) => {
                    let now = self.queue.database_now().await?;
                    let retry_at = now + retry_delay(job.attempts);
                    match self
                        .queue
                        .repository()
                        .fail(
                            self.queue.primary(),
                            job.id,
                            worker_id,
                            retry_at,
                            &error.to_string(),
                            now,
                        )
                        .await?
                    {
                        JobFailureDisposition::Retried { available_at } => {
                            tracing::debug!(
                                job_id = job.id,
                                job_type = %job.job_type,
                                worker_id,
                                attempts = job.attempts,
                                max_attempts = job.max_attempts,
                                retry_at = %available_at,
                                error = %error,
                                "后台任务执行失败，已安排重试"
                            );
                            Ok(JobRunResult::Retried)
                        }
                        JobFailureDisposition::Dead => {
                            tracing::error!(
                                job_id = job.id,
                                job_type = %job.job_type,
                                worker_id,
                                attempts = job.attempts,
                                max_attempts = job.max_attempts,
                                error = %error,
                                "后台任务重试耗尽，已进入死信状态"
                            );
                            Ok(JobRunResult::Dead)
                        }
                        JobFailureDisposition::LeaseLost => Ok(JobRunResult::LeaseLost),
                    }
                }
            }
        }
        .instrument(span)
        .await
    }

    async fn run_until_shutdown(&self, worker_id: String, mut shutdown: watch::Receiver<bool>) {
        tracing::info!(worker_id = %worker_id, "后台任务 Worker 已启动");
        let mut consecutive_infrastructure_failures = 0_u32;
        loop {
            if *shutdown.borrow() {
                break;
            }

            match self.run_once(&worker_id).await {
                Ok(JobRunResult::Idle) => {
                    if consecutive_infrastructure_failures > 0 {
                        tracing::info!(worker_id = %worker_id, "后台任务 Worker 基础设施调用已恢复");
                    }
                    consecutive_infrastructure_failures = 0;
                    tokio::select! {
                        _ = time::sleep(self.poll_interval) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                    }
                }
                Ok(JobRunResult::LeaseLost) => {
                    if consecutive_infrastructure_failures > 0 {
                        tracing::info!(worker_id = %worker_id, "后台任务 Worker 基础设施调用已恢复");
                    }
                    consecutive_infrastructure_failures = 0;
                    tracing::warn!(worker_id = %worker_id, "后台任务租约已失效，忽略本次处理结果");
                }
                Ok(_) => {
                    if consecutive_infrastructure_failures > 0 {
                        tracing::info!(worker_id = %worker_id, "后台任务 Worker 基础设施调用已恢复");
                    }
                    consecutive_infrastructure_failures = 0;
                }
                Err(error) => {
                    consecutive_infrastructure_failures =
                        consecutive_infrastructure_failures.saturating_add(1);
                    let delay = infrastructure_retry_delay(
                        self.poll_interval,
                        consecutive_infrastructure_failures,
                    );
                    if consecutive_infrastructure_failures == 1 {
                        tracing::warn!(
                            worker_id = %worker_id,
                            error = %error,
                            delay_ms = delay.as_millis(),
                            "后台任务 Worker 基础设施调用失败，将退避后重试"
                        );
                    } else {
                        tracing::debug!(
                            worker_id = %worker_id,
                            error = %error,
                            consecutive_infrastructure_failures,
                            delay_ms = delay.as_millis(),
                            "后台任务 Worker 基础设施调用仍不可用"
                        );
                    }
                    tokio::select! {
                        _ = time::sleep(delay) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        tracing::info!(worker_id = %worker_id, "后台任务 Worker 已停止");
    }

    /// 单独回收过期租约，避免与并发领取任务共享同一事务。
    async fn recover_expired_leases_until_shutdown(&self, mut shutdown: watch::Receiver<bool>) {
        let mut recovery_degraded = false;
        loop {
            if *shutdown.borrow() {
                break;
            }
            match self.queue.recover_expired_leases().await {
                Ok(()) if recovery_degraded => {
                    tracing::info!("后台任务过期租约回收已恢复");
                    recovery_degraded = false;
                }
                Ok(()) => {}
                Err(error) if recovery_degraded => {
                    tracing::debug!(%error, "后台任务过期租约回收仍不可用");
                }
                Err(error) => {
                    tracing::warn!(%error, "后台任务过期租约回收失败，将在下次轮询重试");
                    recovery_degraded = true;
                }
            }
            tokio::select! {
                _ = time::sleep(self.poll_interval) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }
}

/// 将任务执行结果映射为固定的低基数指标标签。
fn job_run_result_label(result: &AppResult<JobRunResult>) -> &'static str {
    match result {
        Ok(JobRunResult::Succeeded) => "succeeded",
        Ok(JobRunResult::Retried) => "retried",
        Ok(JobRunResult::Dead) => "dead",
        Ok(JobRunResult::LeaseLost) => "lease_lost",
        Ok(JobRunResult::Idle) => "idle",
        Err(_) => "error",
    }
}

/// 将未注册任务归并到固定标签，避免异常数据扩大 Prometheus 标签基数。
fn bounded_job_type_label(registered: bool, job_type: &str) -> &str {
    if registered { job_type } else { "unregistered" }
}

/// 生成指数退避等待时间，最高五分钟。
pub(super) fn retry_delay(attempts: i32) -> Duration {
    let exponent = attempts.saturating_sub(1).clamp(0, 6) as u32;
    let seconds = 5_i64.saturating_mul(1_i64 << exponent).min(300);
    Duration::seconds(seconds)
}

/// 计算基础设施调用连续失败后的本地退避时间，避免数据库故障时形成高频请求和日志。
pub(super) fn infrastructure_retry_delay(
    poll_interval: StdDuration,
    consecutive_failures: u32,
) -> StdDuration {
    const MAX_DELAY: StdDuration = StdDuration::from_secs(30);
    let exponent = consecutive_failures.saturating_sub(1).min(30);
    let multiplier = 1_u32 << exponent;
    poll_interval.saturating_mul(multiplier).min(MAX_DELAY)
}
