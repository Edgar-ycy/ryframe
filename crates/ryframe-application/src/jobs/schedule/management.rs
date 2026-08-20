use super::*;

impl JobScheduleService {
    pub fn new(
        persistence: Arc<dyn JobSchedulePersistencePort>,
        queue: Arc<JobQueue>,
        execution_tenant_scope: ExecutionTenantScope,
        targets: ScheduledJobTargetRegistry,
        policy: crate::JobSchedulePolicy,
    ) -> Self {
        Self {
            persistence,
            queue,
            execution_tenant_scope,
            targets,
            metrics: None,
            poll_interval: policy.poll_interval,
            batch_size: policy.batch_size,
            max_enabled_per_tenant: policy.max_enabled_per_tenant,
        }
    }

    /// 绑定 Cron 专用监控观察者，不把调度指标注入通用任务队列。
    pub fn with_metrics_observer(mut self, observer: Arc<dyn ScheduleMetricsObserver>) -> Self {
        self.metrics = Some(observer);
        self
    }

    pub fn targets_for_tenant(
        &self,
        actor: &ActorContext,
    ) -> AppResult<Vec<ScheduledJobTargetDescriptor>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        Ok(self.targets.descriptors_for_tenant(tenant_id))
    }

    /// 返回启动阶段用于处理器一致性校验的调度目标注册表。
    pub fn target_registry(&self) -> &ScheduledJobTargetRegistry {
        &self.targets
    }

    pub(super) fn record_scan(&self, result: &'static str) {
        if let Some(observer) = self.metrics.as_ref() {
            observer.record_scan(result);
        }
    }

    pub(super) fn record_trigger(&self, outcome: &'static str) {
        if let Some(observer) = self.metrics.as_ref() {
            observer.record_trigger(outcome);
        }
    }

    pub(super) fn observe_lag(&self, lag: StdDuration) {
        if let Some(observer) = self.metrics.as_ref() {
            observer.observe_lag(lag);
        }
    }

    pub async fn preview(
        &self,
        cron_expression: &str,
        timezone: &str,
    ) -> AppResult<JobSchedulePreview> {
        let parsed = ParsedSchedule::parse(cron_expression, timezone)?;
        let now = self.persistence.database_now().await?;
        let occurrences = parsed
            .future_occurrences(now, 5)?
            .into_iter()
            .map(|utc| JobScheduleOccurrence {
                utc,
                schedule_time: utc.with_timezone(&parsed.timezone).to_rfc3339(),
            })
            .collect();
        Ok(JobSchedulePreview {
            calculated_at: now,
            timezone: parsed.timezone.name().to_owned(),
            occurrences,
        })
    }

    pub async fn list(
        &self,
        actor: &ActorContext,
        params: JobScheduleListParams,
    ) -> AppResult<PageResult<JobScheduleVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let name = normalize_optional(&params.name, MAX_NAME_BYTES, "计划名称")?;
        let handler_key = normalize_optional(&params.handler_key, 96, "处理器标识")?;
        let page = self
            .persistence
            .page(
                tenant_id,
                JobScheduleReadFilter {
                    name: name.as_deref(),
                    handler_key: handler_key.as_deref(),
                    enabled: params.enabled,
                },
                params.page,
            )
            .await?;
        Ok(PageResult {
            records: page.records.into_iter().map(JobScheduleVo::from).collect(),
            total: page.total,
            page: page.page,
            page_size: page.page_size,
        })
    }

    pub async fn get(&self, actor: &ActorContext, id: i64) -> AppResult<JobScheduleVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        validate_id(id)?;
        self.persistence
            .find(tenant_id, id)
            .await?
            .map(JobScheduleVo::from)
            .ok_or_else(|| AppError::NotFound("定时任务不存在".into()))
    }

    pub async fn create(
        &self,
        actor: &ActorContext,
        command: CreateJobSchedule,
    ) -> AppResult<JobScheduleVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let validated = self.validate_command(tenant_id, command)?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_tenant(tenant_id).await?;
        if validated.enabled {
            self.ensure_enabled_limit(transaction.as_ref(), tenant_id)
                .await?;
        }
        let now = transaction.database_now().await?;
        let next_run_at = if validated.enabled {
            Some(validated.parsed.next_after(now)?)
        } else {
            None
        };
        let schedule = transaction
            .insert_schedule(JobScheduleRecord {
                id: crate::next_id()?,
                tenant_id: tenant_id.to_owned(),
                name: validated.name,
                handler_key: validated.handler_key,
                cron_expression: validated.cron_expression,
                timezone: validated.timezone,
                enabled: validated.enabled,
                misfire_policy: validated.misfire_policy,
                concurrency_policy: validated.concurrency_policy,
                max_runtime_seconds: validated.max_runtime_seconds,
                next_run_at,
                last_run_at: None,
                version: 1,
                created_at: now,
                updated_at: now,
                deleted: false,
            })
            .await?;
        transaction.commit().await?;
        Ok(schedule.into())
    }

    pub async fn update(
        &self,
        actor: &ActorContext,
        id: i64,
        command: UpdateJobSchedule,
    ) -> AppResult<JobScheduleVo> {
        validate_id(id)?;
        validate_version(command.version)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let validated = self.validate_command(
            tenant_id,
            CreateJobSchedule {
                name: command.name,
                handler_key: command.handler_key,
                cron_expression: command.cron_expression,
                timezone: command.timezone,
                enabled: command.enabled,
                misfire_policy: command.misfire_policy,
                concurrency_policy: command.concurrency_policy,
                max_runtime_seconds: command.max_runtime_seconds,
            },
        )?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_tenant(tenant_id).await?;
        let mut current = transaction
            .lock_schedule(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("定时任务不存在".into()))?;
        if current.version != command.version {
            return rollback_with(
                transaction,
                AppError::Conflict("定时任务已被其他管理员修改，请刷新后重试".into()),
            )
            .await;
        }
        if validated.enabled && !current.enabled {
            self.ensure_enabled_limit(transaction.as_ref(), tenant_id)
                .await?;
        }
        let now = transaction.database_now().await?;
        let next_run_at = if validated.enabled {
            Some(validated.parsed.next_after(now)?)
        } else {
            None
        };
        current.name = validated.name;
        current.handler_key = validated.handler_key;
        current.cron_expression = validated.cron_expression;
        current.timezone = validated.timezone;
        current.enabled = validated.enabled;
        current.misfire_policy = validated.misfire_policy;
        current.concurrency_policy = validated.concurrency_policy;
        current.max_runtime_seconds = validated.max_runtime_seconds;
        current.next_run_at = next_run_at;
        current.version = command.version + 1;
        current.updated_at = now;
        let updated = transaction.save_schedule(current).await?;
        transaction.commit().await?;
        Ok(updated.into())
    }

    pub async fn set_enabled(
        &self,
        actor: &ActorContext,
        id: i64,
        version: i64,
        enabled: bool,
    ) -> AppResult<JobScheduleVo> {
        validate_id(id)?;
        validate_version(version)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        transaction.lock_tenant(tenant_id).await?;
        let mut current = transaction
            .lock_schedule(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("定时任务不存在".into()))?;
        if current.version != version {
            return rollback_with(
                transaction,
                AppError::Conflict("定时任务状态已变化，请刷新后重试".into()),
            )
            .await;
        }
        if enabled && !current.enabled {
            self.ensure_enabled_limit(transaction.as_ref(), tenant_id)
                .await?;
        }
        let now = transaction.database_now().await?;
        let next_run_at = if enabled {
            Some(
                ParsedSchedule::parse(&current.cron_expression, &current.timezone)?
                    .next_after(now)?,
            )
        } else {
            None
        };
        current.enabled = enabled;
        current.next_run_at = next_run_at;
        current.version = version + 1;
        current.updated_at = now;
        let updated = transaction.save_schedule(current).await?;
        transaction.commit().await?;
        Ok(updated.into())
    }

    pub async fn remove(&self, actor: &ActorContext, id: i64, version: i64) -> AppResult<()> {
        validate_id(id)?;
        validate_version(version)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.persistence.begin().await?;
        let mut current = transaction
            .lock_schedule(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("定时任务不存在".into()))?;
        if current.version != version {
            return rollback_with(
                transaction,
                AppError::Conflict("定时任务已变化，请刷新后重试".into()),
            )
            .await;
        }
        let now = transaction.database_now().await?;
        current.enabled = false;
        current.next_run_at = None;
        current.deleted = true;
        current.version = version + 1;
        current.updated_at = now;
        transaction.save_schedule(current).await?;
        transaction.commit().await
    }

    pub async fn run_now(
        &self,
        actor: &ActorContext,
        id: i64,
        idempotency_key: &str,
    ) -> AppResult<JobScheduleExecutionVo> {
        validate_id(id)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let idempotency_key = normalize_idempotency_key(idempotency_key)?;
        let fire_key = manual_fire_key(idempotency_key);
        let transaction = self.persistence.begin().await?;
        let mut schedule = transaction
            .lock_schedule(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("定时任务不存在".into()))?;
        if let Some(existing) = transaction
            .find_execution_by_fire_key(id, &fire_key)
            .await?
        {
            transaction.commit().await?;
            return Ok(existing.into());
        }
        let target = self.resolve_target(tenant_id, &schedule.handler_key, true)?;
        if schedule.concurrency_policy == CONCURRENCY_FORBID
            && transaction.has_active_job(id).await?
        {
            return rollback_with(
                transaction,
                AppError::Conflict("该计划已有待执行或运行中的任务".into()),
            )
            .await;
        }
        let now = transaction.database_now().await?;
        let context = ScheduledJobContext {
            tenant_id,
            schedule_id: schedule.id,
            trigger_kind: TRIGGER_MANUAL,
            scheduled_for: now,
            max_runtime_seconds: schedule.max_runtime_seconds,
            fire_key: &fire_key,
        };
        let job = target.build_job(&context)?;
        let execution = transaction
            .insert_execution(
                &schedule,
                NewJobScheduleExecution {
                    id: crate::next_id()?,
                    fire_key,
                    trigger_kind: TRIGGER_MANUAL.to_owned(),
                    scheduled_for: now,
                    outcome: OUTCOME_ENQUEUED.to_owned(),
                    detail: None,
                    created_at: now,
                },
            )
            .await?;
        let result = transaction.enqueue(job).await?;
        let execution = transaction
            .attach_background_job(execution, result.job_id)
            .await?;
        schedule.last_run_at = Some(now);
        schedule.updated_at = now;
        transaction.save_schedule(schedule).await?;
        transaction.commit().await?;
        self.record_trigger(OUTCOME_ENQUEUED);
        self.queue.notify_background_jobs().await;
        Ok(execution.into())
    }

    pub async fn executions(
        &self,
        actor: &ActorContext,
        schedule_id: i64,
        params: JobScheduleExecutionListParams,
    ) -> AppResult<PageResult<JobScheduleExecutionVo>> {
        validate_id(schedule_id)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.persistence
            .find(tenant_id, schedule_id)
            .await?
            .ok_or_else(|| AppError::NotFound("定时任务不存在".into()))?;
        let trigger_kind = normalize_trigger_kind(params.trigger_kind)?;
        let outcome = normalize_outcome(params.outcome)?;
        let background_status = normalize_background_status(params.background_job_status)?;
        let page = self
            .persistence
            .execution_page(
                tenant_id,
                schedule_id,
                JobScheduleExecutionReadFilter {
                    trigger_kind: trigger_kind.as_deref(),
                    outcome: outcome.as_deref(),
                    background_job_status: background_status.as_deref(),
                },
                params.page,
            )
            .await?;
        Ok(PageResult {
            records: page.records.into_iter().map(Into::into).collect(),
            total: page.total,
            page: page.page,
            page_size: page.page_size,
        })
    }

    fn validate_command(
        &self,
        tenant_id: &str,
        command: CreateJobSchedule,
    ) -> AppResult<ValidatedScheduleCommand> {
        let name = normalize_required(&command.name, MAX_NAME_BYTES, "计划名称")?;
        let handler_key = normalize_required(&command.handler_key, 96, "处理器标识")?;
        self.resolve_target(tenant_id, &handler_key, true)?;
        let parsed = ParsedSchedule::parse(&command.cron_expression, &command.timezone)?;
        let misfire_policy = match command.misfire_policy.as_str() {
            MISFIRE_SKIP | MISFIRE_FIRE_ONCE => command.misfire_policy,
            _ => {
                return Err(AppError::Validation(
                    "错过执行策略只能是 skip 或 fire_once".into(),
                ));
            }
        };
        let concurrency_policy = match command.concurrency_policy.as_str() {
            CONCURRENCY_FORBID | CONCURRENCY_ALLOW => command.concurrency_policy,
            _ => {
                return Err(AppError::Validation(
                    "并发策略只能是 forbid 或 allow".into(),
                ));
            }
        };
        if !(1..=86_400).contains(&command.max_runtime_seconds) {
            return Err(AppError::Validation(
                "最大运行时长必须在 1 到 86400 秒之间".into(),
            ));
        }
        Ok(ValidatedScheduleCommand {
            name,
            handler_key,
            cron_expression: parsed.expression.clone(),
            timezone: parsed.timezone.name().to_owned(),
            enabled: command.enabled,
            misfire_policy,
            concurrency_policy,
            max_runtime_seconds: command.max_runtime_seconds,
            parsed,
        })
    }

    pub(super) fn resolve_target(
        &self,
        tenant_id: &str,
        handler_key: &str,
        require_available: bool,
    ) -> AppResult<Arc<dyn ScheduledJobTarget>> {
        let target = self
            .targets
            .get(handler_key)
            .ok_or_else(|| AppError::Validation("未知的调度目标".into()))?;
        if target.scope() == ScheduledJobTargetScope::System && tenant_id != SYSTEM_TENANT_ID {
            return Err(AppError::Authorization(
                "当前租户不能使用平台维护调度目标".into(),
            ));
        }
        if require_available && !target.available() {
            return Err(AppError::ServiceUnavailable(
                "当前配置下调度目标不可用".into(),
            ));
        }
        if !target.available() {
            return Err(AppError::ServiceUnavailable(
                "当前配置下调度目标不可用".into(),
            ));
        }
        Ok(target)
    }

    async fn ensure_enabled_limit(
        &self,
        transaction: &dyn JobScheduleTransaction,
        tenant_id: &str,
    ) -> AppResult<()> {
        let current = transaction.count_enabled(tenant_id).await?;
        if current >= self.max_enabled_per_tenant as u64 {
            return Err(AppError::Conflict(format!(
                "当前租户最多启用 {} 个定时任务",
                self.max_enabled_per_tenant
            )));
        }
        Ok(())
    }
}
