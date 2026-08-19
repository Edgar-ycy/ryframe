use super::*;

impl JobScheduleService {
    pub fn new(
        database: ControlDatabaseCluster,
        queue: Arc<JobQueue>,
        execution_tenant_scope: ExecutionTenantScope,
        targets: ScheduledJobTargetRegistry,
        policy: crate::JobSchedulePolicy,
    ) -> Self {
        Self {
            database,
            repository: Arc::new(JobScheduleRepository),
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
        let now = self.queue.database_now().await?;
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
            .repository
            .list(
                self.database.write(),
                tenant_id,
                JobScheduleFilter {
                    name: name.as_deref(),
                    handler_key: handler_key.as_deref(),
                    enabled: params.enabled,
                },
                &params.page,
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
        self.repository
            .find_for_tenant(self.database.write(), tenant_id, id)
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
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        lock_tenant(&transaction, tenant_id).await?;
        if validated.enabled {
            self.ensure_enabled_limit(&transaction, tenant_id).await?;
        }
        let now = self.repository_clock(&transaction).await?;
        let next_run_at = if validated.enabled {
            Some(validated.parsed.next_after(now)?)
        } else {
            None
        };
        let schedule = self
            .repository
            .insert(
                &transaction,
                job_schedule::ActiveModel {
                    id: Set(snowflake::try_next_snowflake_id()?),
                    tenant_id: Set(tenant_id.to_owned()),
                    name: Set(validated.name),
                    handler_key: Set(validated.handler_key),
                    cron_expression: Set(validated.cron_expression),
                    timezone: Set(validated.timezone),
                    enabled: Set(validated.enabled),
                    misfire_policy: Set(validated.misfire_policy),
                    concurrency_policy: Set(validated.concurrency_policy),
                    max_runtime_seconds: Set(validated.max_runtime_seconds),
                    next_run_at: Set(next_run_at),
                    last_run_at: Set(None),
                    version: Set(1),
                    del_flag: Set(job_schedule::Model::DEL_FLAG_NORMAL.to_owned()),
                    created_at: Set(now),
                    updated_at: Set(now),
                },
            )
            .await?;
        transaction.commit().await.map_err(database_error)?;
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
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        lock_tenant(&transaction, tenant_id).await?;
        let current = self
            .repository
            .lock_for_tenant(&transaction, tenant_id, id)
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
            self.ensure_enabled_limit(&transaction, tenant_id).await?;
        }
        let now = self.repository_clock(&transaction).await?;
        let next_run_at = if validated.enabled {
            Some(validated.parsed.next_after(now)?)
        } else {
            None
        };
        let mut active: job_schedule::ActiveModel = current.into();
        active.name = Set(validated.name);
        active.handler_key = Set(validated.handler_key);
        active.cron_expression = Set(validated.cron_expression);
        active.timezone = Set(validated.timezone);
        active.enabled = Set(validated.enabled);
        active.misfire_policy = Set(validated.misfire_policy);
        active.concurrency_policy = Set(validated.concurrency_policy);
        active.max_runtime_seconds = Set(validated.max_runtime_seconds);
        active.next_run_at = Set(next_run_at);
        active.version = Set(command.version + 1);
        active.updated_at = Set(now);
        let updated = active.update(&transaction).await.map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
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
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        lock_tenant(&transaction, tenant_id).await?;
        let current = self
            .repository
            .lock_for_tenant(&transaction, tenant_id, id)
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
            self.ensure_enabled_limit(&transaction, tenant_id).await?;
        }
        let now = self.repository_clock(&transaction).await?;
        let next_run_at = if enabled {
            Some(
                ParsedSchedule::parse(&current.cron_expression, &current.timezone)?
                    .next_after(now)?,
            )
        } else {
            None
        };
        let mut active: job_schedule::ActiveModel = current.into();
        active.enabled = Set(enabled);
        active.next_run_at = Set(next_run_at);
        active.version = Set(version + 1);
        active.updated_at = Set(now);
        let updated = active.update(&transaction).await.map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(updated.into())
    }

    pub async fn remove(&self, actor: &ActorContext, id: i64, version: i64) -> AppResult<()> {
        validate_id(id)?;
        validate_version(version)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        let current = self
            .repository
            .lock_for_tenant(&transaction, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("定时任务不存在".into()))?;
        if current.version != version {
            return rollback_with(
                transaction,
                AppError::Conflict("定时任务已变化，请刷新后重试".into()),
            )
            .await;
        }
        let now = self.repository_clock(&transaction).await?;
        let mut active: job_schedule::ActiveModel = current.into();
        active.enabled = Set(false);
        active.next_run_at = Set(None);
        active.del_flag = Set(job_schedule::Model::DEL_FLAG_DELETED.to_owned());
        active.version = Set(version + 1);
        active.updated_at = Set(now);
        active.update(&transaction).await.map_err(database_error)?;
        transaction.commit().await.map_err(database_error)
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
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        let schedule = self
            .repository
            .lock_for_tenant(&transaction, tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("定时任务不存在".into()))?;
        if let Some(existing) = self
            .repository
            .find_execution_by_fire_key(&transaction, id, &fire_key)
            .await?
        {
            transaction.commit().await.map_err(database_error)?;
            return self.execution_vo(existing).await;
        }
        let target = self.resolve_target(tenant_id, &schedule.handler_key, true)?;
        if schedule.concurrency_policy == job_schedule::Model::CONCURRENCY_FORBID
            && self.repository.has_active_job(&transaction, id).await?
        {
            return rollback_with(
                transaction,
                AppError::Conflict("该计划已有待执行或运行中的任务".into()),
            )
            .await;
        }
        let now = self.repository_clock(&transaction).await?;
        let execution = insert_execution(
            &transaction,
            &schedule,
            NewExecution {
                fire_key: &fire_key,
                trigger_kind: job_schedule_execution::Model::TRIGGER_MANUAL,
                scheduled_for: now,
                outcome: job_schedule_execution::Model::OUTCOME_ENQUEUED,
                detail: None,
                created_at: now,
            },
        )
        .await?;
        let context = ScheduledJobContext {
            tenant_id,
            schedule_id: schedule.id,
            trigger_kind: job_schedule_execution::Model::TRIGGER_MANUAL,
            scheduled_for: now,
            max_runtime_seconds: schedule.max_runtime_seconds,
            fire_key: &fire_key,
        };
        let result = self
            .queue
            .enqueue_in_transaction(&transaction, target.build_job(&context)?)
            .await?;
        let execution = attach_background_job(&transaction, execution, result.job.id).await?;
        let mut active: job_schedule::ActiveModel = schedule.into();
        active.last_run_at = Set(Some(now));
        active.updated_at = Set(now);
        active.update(&transaction).await.map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        self.record_trigger(job_schedule_execution::Model::OUTCOME_ENQUEUED);
        self.queue.notify_background_jobs().await;
        self.execution_vo(execution).await
    }

    pub async fn executions(
        &self,
        actor: &ActorContext,
        schedule_id: i64,
        params: JobScheduleExecutionListParams,
    ) -> AppResult<PageResult<JobScheduleExecutionVo>> {
        validate_id(schedule_id)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.repository
            .find_for_tenant(self.database.write(), tenant_id, schedule_id)
            .await?
            .ok_or_else(|| AppError::NotFound("定时任务不存在".into()))?;
        let trigger_kind = normalize_trigger_kind(params.trigger_kind)?;
        let outcome = normalize_outcome(params.outcome)?;
        let background_status = normalize_background_status(params.background_job_status)?;
        let page = self
            .repository
            .list_executions(
                self.database.write(),
                tenant_id,
                schedule_id,
                JobScheduleExecutionFilter {
                    trigger_kind: trigger_kind.as_deref(),
                    outcome: outcome.as_deref(),
                    background_job_status: background_status.as_deref(),
                },
                &params.page,
            )
            .await?;
        let ids = page
            .records
            .iter()
            .filter_map(|execution| execution.background_job_id)
            .collect::<Vec<_>>();
        let statuses = self
            .repository
            .background_job_statuses(self.database.write(), &ids)
            .await?;
        let records = page
            .records
            .into_iter()
            .map(|execution| {
                let status = execution
                    .background_job_id
                    .and_then(|id| statuses.get(&id).cloned());
                execution_into_vo(execution, status)
            })
            .collect::<Vec<_>>();
        Ok(PageResult {
            records,
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
            job_schedule::Model::MISFIRE_SKIP | job_schedule::Model::MISFIRE_FIRE_ONCE => {
                command.misfire_policy
            }
            _ => {
                return Err(AppError::Validation(
                    "错过执行策略只能是 skip 或 fire_once".into(),
                ));
            }
        };
        let concurrency_policy = match command.concurrency_policy.as_str() {
            job_schedule::Model::CONCURRENCY_FORBID | job_schedule::Model::CONCURRENCY_ALLOW => {
                command.concurrency_policy
            }
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

    async fn ensure_enabled_limit<C>(&self, db: &C, tenant_id: &str) -> AppResult<()>
    where
        C: ConnectionTrait,
    {
        let current = self.repository.count_enabled(db, tenant_id).await?;
        if current >= self.max_enabled_per_tenant as u64 {
            return Err(AppError::Conflict(format!(
                "当前租户最多启用 {} 个定时任务",
                self.max_enabled_per_tenant
            )));
        }
        Ok(())
    }

    pub(super) async fn repository_clock<C>(&self, db: &C) -> AppResult<DateTime<Utc>>
    where
        C: ConnectionTrait,
    {
        self.queue.repository().database_utc_now(db).await
    }

    async fn execution_vo(
        &self,
        execution: job_schedule_execution::Model,
    ) -> AppResult<JobScheduleExecutionVo> {
        let ids = execution.background_job_id.into_iter().collect::<Vec<_>>();
        let statuses = self
            .repository
            .background_job_statuses(self.database.write(), &ids)
            .await?;
        let status = execution
            .background_job_id
            .and_then(|id| statuses.get(&id).cloned());
        Ok(execution_into_vo(execution, status))
    }
}
