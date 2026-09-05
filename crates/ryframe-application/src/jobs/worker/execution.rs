use super::*;

impl JobWorker {
    /// 执行一次领取和处理，供单次执行模式及自定义运行器使用。
    pub async fn run_once(&self, worker_id: &str) -> AppResult<JobRunResult> {
        let now = match self.queue.database_now().await {
            Ok(now) => now,
            Err(error) => {
                self.queue.record_claim_attempt("background_job", "error");
                return Err(error);
            }
        };
        let claimed = match self
            .queue
            .claim_next(
                worker_id,
                self.lease_duration,
                now,
                &self.execution_tenant_scope,
            )
            .await
        {
            Ok(claimed) => claimed,
            Err(error) => {
                self.queue.record_claim_attempt("background_job", "error");
                return Err(error);
            }
        };
        let job = match claimed {
            Some(job) => {
                self.queue.record_claim_attempt("background_job", "claimed");
                job
            }
            None => {
                self.queue.record_claim_attempt("background_job", "idle");
                return Ok(JobRunResult::Idle);
            }
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
        mut job: ClaimedJobRecord,
        worker_id: &str,
    ) -> AppResult<JobRunResult> {
        let Some(handler) = self.handlers.get(&job.job_type).cloned() else {
            return self.reject_unregistered_job(&job, worker_id).await;
        };

        let span = tracing::info_span!("background_job", job_type = %job.job_type);
        crate::trace_context::set_parent(
            &span,
            job.traceparent.as_deref(),
            job.tracestate.as_deref(),
        );
        let claimed_job = ClaimedBackgroundJob {
            id: job.id,
            tenant_id: job.tenant_id.take(),
            payload: std::mem::take(&mut job.payload),
            lease_owner: job.lease_owner.take(),
            attempts: job.attempts,
            max_attempts: job.max_attempts,
        };
        async {
            let heartbeat_queue = self.queue.clone();
            let heartbeat_worker_id = worker_id.to_owned();
            let heartbeat_job_id = job.id;
            let lease_duration = self.lease_duration;
            let operation = async {
                if let Some(seconds) = job.max_runtime_seconds {
                    let seconds = u64::try_from(seconds)
                        .map_err(|_| AppError::Internal("计划任务最大运行时长不是正整数".into()))?;
                    match time::timeout(
                        StdDuration::from_secs(seconds),
                        handler.handle(&claimed_job),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err(AppError::ServiceUnavailable(format!(
                            "计划任务执行超过最大运行时长 {seconds} 秒"
                        ))),
                    }
                } else {
                    handler.handle(&claimed_job).await
                }
            };
            let handler_result =
                match run_with_lease_heartbeat(operation, self.heartbeat_interval, move || {
                    let queue = heartbeat_queue.clone();
                    let worker_id = heartbeat_worker_id.clone();
                    async move {
                        let now = queue.database_now().await?;
                        queue
                            .renew_lease(heartbeat_job_id, &worker_id, lease_duration, now)
                            .await
                    }
                })
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
                    let completed = self.queue.complete(job.id, worker_id, now).await?;
                    Ok(if completed {
                        JobRunResult::Succeeded
                    } else {
                        JobRunResult::LeaseLost
                    })
                }
                Err(error) => {
                    self.finish_failed_job(&job, worker_id, handler.as_ref(), error)
                        .await
                }
            }
        }
        .instrument(span)
        .await
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

fn job_run_result_for_failure(outcome: &JobFailureOutcome) -> JobRunResult {
    match outcome {
        JobFailureOutcome::Retried { .. } => JobRunResult::Retried,
        JobFailureOutcome::Dead => JobRunResult::Dead,
        JobFailureOutcome::Completed => JobRunResult::Succeeded,
        JobFailureOutcome::LeaseLost => JobRunResult::LeaseLost,
    }
}

/// 将未注册任务归并到固定标签，避免异常数据扩大 Prometheus 标签基数。
fn bounded_job_type_label(registered: bool, job_type: &str) -> &str {
    if registered { job_type } else { "unregistered" }
}

/// 配置迁移任务可能处理上传包、数据库和对象路径，日志只记录稳定错误类别，
/// 避免把底层错误详情或配置内容写入普通 Worker 日志。
fn job_log_error(job_type: &str, error: &AppError) -> String {
    if matches!(
        job_type,
        "system.tenant_config.export"
            | "system.tenant_config.preview"
            | "system.tenant_config.apply"
            | "system.tenant_config.rollback"
    ) {
        format!("配置迁移任务失败（错误类别：{}）", error.error_code())
    } else {
        error.to_string()
    }
}

impl JobWorker {
    async fn reject_unregistered_job(
        &self,
        job: &ClaimedJobRecord,
        worker_id: &str,
    ) -> AppResult<JobRunResult> {
        let now = self.queue.database_now().await?;
        let failure_reason = format!("未注册任务处理器: {}", job.job_type);
        let outcome = self
            .queue
            .dead_letter(job.id, worker_id, &failure_reason, now)
            .await?;
        if matches!(outcome, JobFailureOutcome::Dead) {
            tracing::error!(
                job_id = job.id,
                job_type = %job.job_type,
                worker_id,
                attempts = job.attempts,
                max_attempts = job.max_attempts,
                failure_reason,
                "后台任务因未注册处理器进入死信状态"
            );
        }
        Ok(job_run_result_for_failure(&outcome))
    }

    async fn finish_failed_job(
        &self,
        job: &ClaimedJobRecord,
        worker_id: &str,
        handler: &dyn JobHandler,
        error: AppError,
    ) -> AppResult<JobRunResult> {
        let now = self.queue.database_now().await?;
        if let AppError::RetryableConflict(message, retry_after_seconds) = &error {
            let retry_after_seconds = (*retry_after_seconds).clamp(1, 86_400);
            let available_at =
                now + Duration::seconds(i64::try_from(retry_after_seconds).unwrap_or(86_400));
            let outcome = self
                .queue
                .defer_retryable_conflict(job.id, worker_id, available_at, message, now)
                .await?;
            if matches!(outcome, JobFailureOutcome::Retried { .. }) {
                tracing::debug!(
                    job_id = job.id,
                    job_type = %job.job_type,
                    worker_id,
                    retry_at = %available_at,
                    "后台任务因资源暂时被占用而延期，未消耗尝试预算"
                );
            }
            return Ok(job_run_result_for_failure(&outcome));
        }
        let retry_at = now + retry_delay(job.attempts);
        let force_dead = handler.should_dead_letter(&error);
        let error_message = error.to_string();
        let log_error = job_log_error(&job.job_type, &error);
        let outcome = self
            .queue
            .fail(FailJobCommand {
                job_id: job.id,
                worker_id,
                retry_at,
                error_message: &error_message,
                force_dead,
                now,
            })
            .await?;
        match &outcome {
            JobFailureOutcome::Retried { available_at } => {
                tracing::debug!(
                    job_id = job.id,
                    job_type = %job.job_type,
                    worker_id,
                    attempts = job.attempts,
                    max_attempts = job.max_attempts,
                    retry_at = %available_at,
                    error = %log_error,
                    "后台任务执行失败，已安排重试"
                );
            }
            JobFailureOutcome::Dead => {
                tracing::error!(
                    job_id = job.id,
                    job_type = %job.job_type,
                    worker_id,
                    attempts = job.attempts,
                    max_attempts = job.max_attempts,
                    error = %log_error,
                    "后台任务重试耗尽，已进入死信状态"
                );
            }
            JobFailureOutcome::Completed => {
                tracing::debug!(
                    job_id = job.id,
                    job_type = %job.job_type,
                    worker_id,
                    "关联业务已经权威终结，后台任务按成功完成收口"
                );
            }
            JobFailureOutcome::LeaseLost => {}
        }
        Ok(job_run_result_for_failure(&outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_outcomes_map_to_worker_results() {
        let retry_at = chrono::Utc::now();
        for (outcome, expected) in [
            (
                JobFailureOutcome::Retried {
                    available_at: retry_at,
                },
                JobRunResult::Retried,
            ),
            (JobFailureOutcome::Dead, JobRunResult::Dead),
            (JobFailureOutcome::Completed, JobRunResult::Succeeded),
            (JobFailureOutcome::LeaseLost, JobRunResult::LeaseLost),
        ] {
            assert_eq!(job_run_result_for_failure(&outcome), expected);
        }
    }
}
