use super::*;

impl JobScheduleService {
    /// 扫描一批到期计划；每条计划在独立事务中使用跳过锁领取。
    pub async fn scan_due_once(&self) -> AppResult<usize> {
        let mut triggered = 0;
        for _ in 0..self.batch_size {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            let now = self.repository_clock(&transaction).await?;
            let Some(schedule) = self.repository.lock_next_due(&transaction, now).await? else {
                transaction.commit().await.map_err(database_error)?;
                break;
            };
            let result = match self.validate_persisted_schedule(&schedule, now) {
                Ok(parsed) => {
                    self.process_due_schedule(&transaction, schedule, now, parsed)
                        .await?
                }
                Err(detail) => {
                    quarantine_invalid_schedule(&transaction, schedule, now, &detail).await?;
                    DueScheduleResult {
                        enqueued: false,
                        outcome: job_schedule_execution::Model::OUTCOME_INVALID_CONFIGURATION,
                    }
                }
            };
            transaction.commit().await.map_err(database_error)?;
            self.record_trigger(result.outcome);
            if result.enqueued {
                triggered += 1;
                self.queue.notify_background_jobs().await;
            }
        }
        self.record_scan("success");
        Ok(triggered)
    }

    pub fn spawn(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                if let Err(error) = self.scan_due_once().await {
                    self.record_scan("error");
                    tracing::warn!(%error, "调度计划扫描失败，将由后续数据库轮询重试");
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
        })
    }

    async fn process_due_schedule(
        &self,
        transaction: &DatabaseTransaction,
        schedule: job_schedule::Model,
        now: DateTime<Utc>,
        parsed: ParsedSchedule,
    ) -> AppResult<DueScheduleResult> {
        let due = schedule
            .next_run_at
            .ok_or_else(|| AppError::Database("已领取计划缺少 next_run_at".into()))?;
        self.observe_lag(
            (now - due)
                .to_std()
                .unwrap_or_else(|_| StdDuration::from_secs(0)),
        );
        let following = parsed.next_after(due)?;
        let misfired = following <= now;
        let next_run_at = if misfired {
            parsed.next_after(now)?
        } else {
            following
        };
        let trigger_kind = if misfired {
            job_schedule_execution::Model::TRIGGER_MISFIRE
        } else {
            job_schedule_execution::Model::TRIGGER_SCHEDULED
        };
        let fire_key = automatic_fire_key(due);

        if misfired && schedule.misfire_policy == job_schedule::Model::MISFIRE_SKIP {
            self.record_non_enqueued_due_execution(
                transaction,
                NonEnqueuedDueExecution {
                    schedule,
                    fire_key: &fire_key,
                    trigger_kind,
                    scheduled_for: due,
                    next_run_at,
                    now,
                    outcome: job_schedule_execution::Model::OUTCOME_SKIPPED_MISFIRE,
                    detail: Some("计划停机期间错过多次触发，已按 skip 策略跳过".into()),
                },
            )
            .await?;
            return Ok(DueScheduleResult {
                enqueued: false,
                outcome: job_schedule_execution::Model::OUTCOME_SKIPPED_MISFIRE,
            });
        }

        let target = match self.resolve_target(&schedule.tenant_id, &schedule.handler_key, false) {
            Ok(target) => target,
            Err(error) => {
                self.record_non_enqueued_due_execution(
                    transaction,
                    NonEnqueuedDueExecution {
                        schedule,
                        fire_key: &fire_key,
                        trigger_kind,
                        scheduled_for: due,
                        next_run_at,
                        now,
                        outcome: job_schedule_execution::Model::OUTCOME_TARGET_UNAVAILABLE,
                        detail: Some(error.to_string()),
                    },
                )
                .await?;
                return Ok(DueScheduleResult {
                    enqueued: false,
                    outcome: job_schedule_execution::Model::OUTCOME_TARGET_UNAVAILABLE,
                });
            }
        };

        if schedule.concurrency_policy == job_schedule::Model::CONCURRENCY_FORBID
            && self
                .repository
                .has_active_job(transaction, schedule.id)
                .await?
        {
            self.record_non_enqueued_due_execution(
                transaction,
                NonEnqueuedDueExecution {
                    schedule,
                    fire_key: &fire_key,
                    trigger_kind,
                    scheduled_for: due,
                    next_run_at,
                    now,
                    outcome: job_schedule_execution::Model::OUTCOME_SKIPPED_CONCURRENCY,
                    detail: Some("同一计划已有待执行或运行中的任务".into()),
                },
            )
            .await?;
            return Ok(DueScheduleResult {
                enqueued: false,
                outcome: job_schedule_execution::Model::OUTCOME_SKIPPED_CONCURRENCY,
            });
        }

        let execution = insert_execution(
            transaction,
            &schedule,
            NewExecution {
                fire_key: &fire_key,
                trigger_kind,
                scheduled_for: due,
                outcome: job_schedule_execution::Model::OUTCOME_ENQUEUED,
                detail: None,
                created_at: now,
            },
        )
        .await?;
        let context = ScheduledJobContext {
            tenant_id: &schedule.tenant_id,
            schedule_id: schedule.id,
            trigger_kind,
            scheduled_for: due,
            max_runtime_seconds: schedule.max_runtime_seconds,
            fire_key: &fire_key,
        };
        let job = self
            .queue
            .enqueue_in_transaction(transaction, target.build_job(&context)?)
            .await?;
        attach_background_job(transaction, execution, job.job.id).await?;
        advance_schedule(transaction, schedule, next_run_at, due, now).await?;
        Ok(DueScheduleResult {
            enqueued: true,
            outcome: job_schedule_execution::Model::OUTCOME_ENQUEUED,
        })
    }

    async fn record_non_enqueued_due_execution(
        &self,
        transaction: &DatabaseTransaction,
        execution: NonEnqueuedDueExecution<'_>,
    ) -> AppResult<()> {
        let NonEnqueuedDueExecution {
            schedule,
            fire_key,
            trigger_kind,
            scheduled_for,
            next_run_at,
            now,
            outcome,
            detail,
        } = execution;
        insert_execution(
            transaction,
            &schedule,
            NewExecution {
                fire_key,
                trigger_kind,
                scheduled_for,
                outcome,
                detail,
                created_at: now,
            },
        )
        .await?;
        advance_schedule(transaction, schedule, next_run_at, scheduled_for, now).await
    }

    fn validate_persisted_schedule(
        &self,
        schedule: &job_schedule::Model,
        now: DateTime<Utc>,
    ) -> Result<ParsedSchedule, String> {
        let target = self
            .targets
            .get(&schedule.handler_key)
            .ok_or_else(|| "未知的调度目标".to_owned())?;
        if target.scope() == ScheduledJobTargetScope::System
            && schedule.tenant_id != SYSTEM_TENANT_ID
        {
            return Err("普通租户不能使用平台维护调度目标".into());
        }
        validate_persisted_schedule(schedule, now)
    }
}

struct NonEnqueuedDueExecution<'a> {
    schedule: job_schedule::Model,
    fire_key: &'a str,
    trigger_kind: &'a str,
    scheduled_for: DateTime<Utc>,
    next_run_at: DateTime<Utc>,
    now: DateTime<Utc>,
    outcome: &'static str,
    detail: Option<String>,
}

struct DueScheduleResult {
    enqueued: bool,
    outcome: &'static str,
}
