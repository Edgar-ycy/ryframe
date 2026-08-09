use std::{str::FromStr, sync::Arc, time::Duration as StdDuration};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use ryframe_config::JobConfig;
use ryframe_core::repository::{PageResult, ValidatedPageQuery};
use ryframe_db::{
    DatabaseCluster, JobScheduleExecutionFilter, JobScheduleFilter, JobScheduleRepository,
    job_schedule, job_schedule_execution,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseTransaction, TransactionTrait,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::{sync::watch, task::JoinHandle, time};

use super::{
    JobQueue, ScheduledJobContext, ScheduledJobTarget, ScheduledJobTargetDescriptor,
    ScheduledJobTargetRegistry, ScheduledJobTargetScope,
};

const SYSTEM_TENANT_ID: &str = "system";
const MAX_NAME_BYTES: usize = 100;
const MAX_CRON_BYTES: usize = 191;
const MAX_TIMEZONE_BYTES: usize = 64;

#[derive(Clone, Debug)]
pub struct JobScheduleListParams {
    pub page: ValidatedPageQuery,
    pub name: Option<String>,
    pub handler_key: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct JobScheduleExecutionListParams {
    pub page: ValidatedPageQuery,
    pub trigger_kind: Option<String>,
    pub outcome: Option<String>,
    pub background_job_status: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CreateJobSchedule {
    pub name: String,
    pub handler_key: String,
    pub cron_expression: String,
    pub timezone: String,
    pub enabled: bool,
    pub misfire_policy: String,
    pub concurrency_policy: String,
    pub max_runtime_seconds: i32,
}

#[derive(Clone, Debug)]
pub struct UpdateJobSchedule {
    pub version: i64,
    pub name: String,
    pub handler_key: String,
    pub cron_expression: String,
    pub timezone: String,
    pub enabled: bool,
    pub misfire_policy: String,
    pub concurrency_policy: String,
    pub max_runtime_seconds: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobScheduleVo {
    pub id: String,
    pub name: String,
    pub handler_key: String,
    pub cron_expression: String,
    pub timezone: String,
    pub enabled: bool,
    pub misfire_policy: String,
    pub concurrency_policy: String,
    pub max_runtime_seconds: i32,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<job_schedule::Model> for JobScheduleVo {
    fn from(schedule: job_schedule::Model) -> Self {
        Self {
            id: schedule.id.to_string(),
            name: schedule.name,
            handler_key: schedule.handler_key,
            cron_expression: schedule.cron_expression,
            timezone: schedule.timezone,
            enabled: schedule.enabled,
            misfire_policy: schedule.misfire_policy,
            concurrency_policy: schedule.concurrency_policy,
            max_runtime_seconds: schedule.max_runtime_seconds,
            next_run_at: schedule.next_run_at,
            last_run_at: schedule.last_run_at,
            version: schedule.version,
            created_at: schedule.created_at,
            updated_at: schedule.updated_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct JobScheduleExecutionVo {
    pub id: String,
    pub schedule_id: String,
    pub schedule_name: String,
    pub handler_key: String,
    pub trigger_kind: String,
    pub scheduled_for: DateTime<Utc>,
    pub outcome: String,
    pub background_job_id: Option<String>,
    pub background_job_status: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobScheduleOccurrence {
    pub utc: DateTime<Utc>,
    pub schedule_time: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobSchedulePreview {
    pub timezone: String,
    pub occurrences: Vec<JobScheduleOccurrence>,
}

/// 数据库驱动的租户调度服务。
#[derive(Clone)]
pub struct JobScheduleService {
    database: DatabaseCluster,
    repository: Arc<JobScheduleRepository>,
    queue: Arc<JobQueue>,
    targets: ScheduledJobTargetRegistry,
    poll_interval: StdDuration,
    batch_size: usize,
    max_enabled_per_tenant: usize,
}

impl JobScheduleService {
    pub fn new(
        database: DatabaseCluster,
        queue: Arc<JobQueue>,
        targets: ScheduledJobTargetRegistry,
        config: &JobConfig,
    ) -> Self {
        Self {
            database,
            repository: Arc::new(JobScheduleRepository),
            queue,
            targets,
            poll_interval: StdDuration::from_millis(config.scheduler_poll_interval_ms),
            batch_size: config.scheduler_batch_size,
            max_enabled_per_tenant: config.max_enabled_schedules_per_tenant,
        }
    }

    pub fn targets_for_tenant(
        &self,
        actor: &ActorContext,
    ) -> AppResult<Vec<ScheduledJobTargetDescriptor>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        Ok(self.targets.descriptors_for_tenant(tenant_id))
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
            let enqueued = self
                .process_due_schedule(&transaction, schedule, now)
                .await?;
            transaction.commit().await.map_err(database_error)?;
            if enqueued {
                triggered += 1;
                self.queue.notify_background_jobs().await;
            }
        }
        self.queue.record_schedule_scan("success");
        Ok(triggered)
    }

    pub fn spawn(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                if let Err(error) = self.scan_due_once().await {
                    self.queue.record_schedule_scan("error");
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
    ) -> AppResult<bool> {
        let due = schedule
            .next_run_at
            .ok_or_else(|| AppError::Database("已领取计划缺少 next_run_at".into()))?;
        self.queue.observe_schedule_lag(
            (now - due)
                .to_std()
                .unwrap_or_else(|_| StdDuration::from_secs(0)),
        );
        let parsed = ParsedSchedule::parse(&schedule.cron_expression, &schedule.timezone)?;
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
            insert_execution(
                transaction,
                &schedule,
                NewExecution {
                    fire_key: &fire_key,
                    trigger_kind,
                    scheduled_for: due,
                    outcome: job_schedule_execution::Model::OUTCOME_SKIPPED_MISFIRE,
                    detail: Some("计划停机期间错过多次触发，已按 skip 策略跳过".into()),
                    created_at: now,
                },
            )
            .await?;
            self.queue
                .record_schedule_trigger(job_schedule_execution::Model::OUTCOME_SKIPPED_MISFIRE);
            advance_schedule(transaction, schedule, next_run_at, due, now).await?;
            return Ok(false);
        }

        let target = match self.resolve_target(&schedule.tenant_id, &schedule.handler_key, false) {
            Ok(target) => target,
            Err(error) => {
                insert_execution(
                    transaction,
                    &schedule,
                    NewExecution {
                        fire_key: &fire_key,
                        trigger_kind,
                        scheduled_for: due,
                        outcome: job_schedule_execution::Model::OUTCOME_TARGET_UNAVAILABLE,
                        detail: Some(error.to_string()),
                        created_at: now,
                    },
                )
                .await?;
                self.queue.record_schedule_trigger(
                    job_schedule_execution::Model::OUTCOME_TARGET_UNAVAILABLE,
                );
                advance_schedule(transaction, schedule, next_run_at, due, now).await?;
                return Ok(false);
            }
        };

        if schedule.concurrency_policy == job_schedule::Model::CONCURRENCY_FORBID
            && self
                .repository
                .has_active_job(transaction, schedule.id)
                .await?
        {
            insert_execution(
                transaction,
                &schedule,
                NewExecution {
                    fire_key: &fire_key,
                    trigger_kind,
                    scheduled_for: due,
                    outcome: job_schedule_execution::Model::OUTCOME_SKIPPED_CONCURRENCY,
                    detail: Some("同一计划已有待执行或运行中的任务".into()),
                    created_at: now,
                },
            )
            .await?;
            self.queue.record_schedule_trigger(
                job_schedule_execution::Model::OUTCOME_SKIPPED_CONCURRENCY,
            );
            advance_schedule(transaction, schedule, next_run_at, due, now).await?;
            return Ok(false);
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
        self.queue
            .record_schedule_trigger(job_schedule_execution::Model::OUTCOME_ENQUEUED);
        advance_schedule(transaction, schedule, next_run_at, due, now).await?;
        Ok(true)
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

    fn resolve_target(
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

    async fn repository_clock<C>(&self, db: &C) -> AppResult<DateTime<Utc>>
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

struct ValidatedScheduleCommand {
    name: String,
    handler_key: String,
    cron_expression: String,
    timezone: String,
    enabled: bool,
    misfire_policy: String,
    concurrency_policy: String,
    max_runtime_seconds: i32,
    parsed: ParsedSchedule,
}

struct ParsedSchedule {
    expression: String,
    timezone: Tz,
    schedule: Schedule,
}

impl ParsedSchedule {
    fn parse(expression: &str, timezone: &str) -> AppResult<Self> {
        let expression = normalize_required(expression, MAX_CRON_BYTES, "Cron 表达式")?;
        let fields = expression.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 7 {
            return Err(AppError::Validation(
                "Cron 表达式必须包含秒、分、时、日、月、周、年七段".into(),
            ));
        }
        if fields[0] != "0" {
            return Err(AppError::Validation(
                "Cron 秒字段首版只允许 0，最小执行间隔为一分钟".into(),
            ));
        }
        if fields[6] != "*" {
            return Err(AppError::Validation("Cron 年字段首版只允许 *".into()));
        }
        let timezone = normalize_required(timezone, MAX_TIMEZONE_BYTES, "时区")?
            .parse::<Tz>()
            .map_err(|_| AppError::Validation("时区必须是有效的 IANA 时区名称".into()))?;
        let schedule = Schedule::from_str(&expression)
            .map_err(|error| AppError::Validation(format!("Cron 表达式无效: {error}")))?;
        Ok(Self {
            expression,
            timezone,
            schedule,
        })
    }

    fn next_after(&self, after: DateTime<Utc>) -> AppResult<DateTime<Utc>> {
        self.schedule
            .after(&after.with_timezone(&self.timezone))
            .next()
            .map(|date| date.with_timezone(&Utc))
            .ok_or_else(|| AppError::Validation("Cron 表达式没有未来执行时间".into()))
    }

    fn future_occurrences(
        &self,
        after: DateTime<Utc>,
        count: usize,
    ) -> AppResult<Vec<DateTime<Utc>>> {
        let occurrences = self
            .schedule
            .after(&after.with_timezone(&self.timezone))
            .take(count)
            .map(|date| date.with_timezone(&Utc))
            .collect::<Vec<_>>();
        if occurrences.len() != count {
            return Err(AppError::Validation(
                "Cron 表达式无法产生足够的未来执行时间".into(),
            ));
        }
        Ok(occurrences)
    }
}

struct NewExecution<'a> {
    fire_key: &'a str,
    trigger_kind: &'a str,
    scheduled_for: DateTime<Utc>,
    outcome: &'a str,
    detail: Option<String>,
    created_at: DateTime<Utc>,
}

async fn insert_execution(
    transaction: &DatabaseTransaction,
    schedule: &job_schedule::Model,
    execution: NewExecution<'_>,
) -> AppResult<job_schedule_execution::Model> {
    job_schedule_execution::ActiveModel {
        id: Set(snowflake::try_next_snowflake_id()?),
        tenant_id: Set(schedule.tenant_id.clone()),
        schedule_id: Set(schedule.id),
        schedule_name_snapshot: Set(schedule.name.clone()),
        handler_key_snapshot: Set(schedule.handler_key.clone()),
        fire_key: Set(execution.fire_key.to_owned()),
        trigger_kind: Set(execution.trigger_kind.to_owned()),
        scheduled_for: Set(execution.scheduled_for),
        outcome: Set(execution.outcome.to_owned()),
        background_job_id: Set(None),
        detail: Set(execution.detail.map(|value| truncate_detail(&value))),
        created_at: Set(execution.created_at),
    }
    .insert(transaction)
    .await
    .map_err(database_error)
}

async fn attach_background_job(
    transaction: &DatabaseTransaction,
    execution: job_schedule_execution::Model,
    background_job_id: i64,
) -> AppResult<job_schedule_execution::Model> {
    let mut active: job_schedule_execution::ActiveModel = execution.into();
    active.background_job_id = Set(Some(background_job_id));
    active.update(transaction).await.map_err(database_error)
}

async fn advance_schedule(
    transaction: &DatabaseTransaction,
    schedule: job_schedule::Model,
    next_run_at: DateTime<Utc>,
    last_run_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let mut active: job_schedule::ActiveModel = schedule.into();
    active.next_run_at = Set(Some(next_run_at));
    active.last_run_at = Set(Some(last_run_at));
    active.updated_at = Set(now);
    active.update(transaction).await.map_err(database_error)?;
    Ok(())
}

async fn lock_tenant(transaction: &DatabaseTransaction, tenant_id: &str) -> AppResult<()> {
    let row = transaction
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::MySql,
            "SELECT tenant_id FROM sys_tenant WHERE tenant_id = ? FOR UPDATE",
            [tenant_id.into()],
        ))
        .await
        .map_err(database_error)?;
    if row.is_none() {
        return Err(AppError::NotFound("当前租户不存在".into()));
    }
    Ok(())
}

fn execution_into_vo(
    execution: job_schedule_execution::Model,
    background_job_status: Option<String>,
) -> JobScheduleExecutionVo {
    JobScheduleExecutionVo {
        id: execution.id.to_string(),
        schedule_id: execution.schedule_id.to_string(),
        schedule_name: execution.schedule_name_snapshot,
        handler_key: execution.handler_key_snapshot,
        trigger_kind: execution.trigger_kind,
        scheduled_for: execution.scheduled_for,
        outcome: execution.outcome,
        background_job_id: execution.background_job_id.map(|id| id.to_string()),
        background_job_status,
        detail: execution.detail,
        created_at: execution.created_at,
    }
}

fn normalize_required(value: &str, max_bytes: usize, label: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > max_bytes {
        return Err(AppError::Validation(format!(
            "{label}必须为 1 到 {max_bytes} 字节"
        )));
    }
    Ok(value.to_owned())
}

fn normalize_optional(
    value: &Option<String>,
    max_bytes: usize,
    label: &str,
) -> AppResult<Option<String>> {
    value
        .as_deref()
        .map(|value| normalize_required(value, max_bytes, label))
        .transpose()
}

fn normalize_trigger_kind(value: Option<String>) -> AppResult<Option<String>> {
    normalize_enum_filter(
        value,
        &[
            job_schedule_execution::Model::TRIGGER_SCHEDULED,
            job_schedule_execution::Model::TRIGGER_MISFIRE,
            job_schedule_execution::Model::TRIGGER_MANUAL,
        ],
        "触发类型",
    )
}

fn normalize_outcome(value: Option<String>) -> AppResult<Option<String>> {
    normalize_enum_filter(
        value,
        &[
            job_schedule_execution::Model::OUTCOME_ENQUEUED,
            job_schedule_execution::Model::OUTCOME_SKIPPED_MISFIRE,
            job_schedule_execution::Model::OUTCOME_SKIPPED_CONCURRENCY,
            job_schedule_execution::Model::OUTCOME_TARGET_UNAVAILABLE,
        ],
        "执行结果",
    )
}

fn normalize_background_status(value: Option<String>) -> AppResult<Option<String>> {
    normalize_enum_filter(
        value,
        &["pending", "running", "succeeded", "dead"],
        "后台任务状态",
    )
}

fn normalize_enum_filter(
    value: Option<String>,
    allowed: &[&str],
    label: &str,
) -> AppResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if !allowed.contains(&value) {
        return Err(AppError::Validation(format!("{label}无效")));
    }
    Ok(Some(value.to_owned()))
}

fn normalize_idempotency_key(value: &str) -> AppResult<&str> {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 {
        return Err(AppError::Validation(
            "Idempotency-Key 必须为 1 到 255 字节".into(),
        ));
    }
    Ok(value)
}

fn manual_fire_key(idempotency_key: &str) -> String {
    format!(
        "manual:{}",
        hex::encode(Sha256::digest(idempotency_key.as_bytes()))
    )
}

fn automatic_fire_key(scheduled_for: DateTime<Utc>) -> String {
    format!("auto:{}", scheduled_for.timestamp_micros())
}

fn validate_id(id: i64) -> AppResult<()> {
    if id <= 0 {
        return Err(AppError::Validation("定时任务 ID 必须是正整数".into()));
    }
    Ok(())
}

fn validate_version(version: i64) -> AppResult<()> {
    if version <= 0 {
        return Err(AppError::Validation("version 必须是正整数".into()));
    }
    Ok(())
}

fn truncate_detail(value: &str) -> String {
    const MAX_BYTES: usize = 2_000;
    if value.len() <= MAX_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

async fn rollback_with<T>(transaction: DatabaseTransaction, error: AppError) -> AppResult<T> {
    if let Err(rollback_error) = transaction.rollback().await {
        tracing::warn!(%rollback_error, "回滚调度事务失败");
    }
    Err(error)
}
