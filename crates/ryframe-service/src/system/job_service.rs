use std::{str::FromStr, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_common::{AppError, AppResult, utils::snowflake};
use ryframe_core::{
    LoggedRepo,
    auto_fill::{AutoFill, FillContext},
    repository::{PageQuery, PageResult, Repository},
};
use ryframe_db::{JobLogRepository, JobRepository, entities::{job, job_log}};
use ryframe_task::{ScheduledTask, TaskHistory, TaskHistoryPersister, TaskScheduler};
use sea_orm::{ActiveModelTrait, DatabaseConnection};
use serde::Serialize;

/// 任务展示 VO
///
/// id 使用 String 而非 i64，避免 Snowflake 64 位 ID 在 JSON 序列化后
/// 超出 JavaScript Number.MAX_SAFE_INTEGER（2^53），导致前端精度丢失。
#[derive(Debug, Serialize)]
pub struct JobVo {
    pub id: String,
    pub name: String,
    pub group_name: String,
    pub cron_expr: String,
    pub misfire_policy: String,
    pub concurrent: String,
    pub status: String,
    pub description: String,
    pub next_fire_time: Option<String>,
    pub remark: Option<String>,
    pub create_time: DateTime<Utc>,
    pub update_time: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct JobLogVo {
    pub id: String,
    pub job_name: String,
    pub job_group: String,
    pub message: String,
    pub status: String,
    pub error_msg: Option<String>,
    pub cost_ms: i64,
    pub start_time: DateTime<Utc>,
}

/// 将 TaskHistory 持久化到 sys_job_log 表
pub struct JobLogPersister {
    pub job_log_repo: LoggedRepo<JobLogRepository>,
    pub db: Arc<DatabaseConnection>,
}

#[async_trait]
impl TaskHistoryPersister for JobLogPersister {
    async fn persist(&self, history: &TaskHistory) -> AppResult<()> {
        let entity = job_log::Model {
            id: snowflake::next_snowflake_id(),
            job_name: history.task_name.clone(),
            job_group: String::new(),
            message: history.message.clone(),
            status: history.status.clone(),
            error_msg: if history.status == TaskHistory::STATUS_FAIL {
                Some(history.message.clone())
            } else {
                None
            },
            cost_ms: history.cost_ms,
            start_time: history.started_at,
        };
        let active: job_log::ActiveModel = entity.into();
        active
            .insert(self.db.as_ref())
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

pub struct JobServiceImpl {
    pub job_repo: LoggedRepo<JobRepository>,
    pub job_log_repo: LoggedRepo<JobLogRepository>,
    pub scheduler: Arc<TaskScheduler>,
}

impl JobServiceImpl {
    /// 初始化内置任务：从数据库读取配置并注册到调度器
    ///
    /// - 如果数据库中已有该任务记录，使用 DB 中的 cron 和 status
    /// - 如果数据库中没有记录，插入默认配置，使用任务自带的默认 cron
    pub async fn init_builtin_tasks(
        &self,
        db: &DatabaseConnection,
        builtin_tasks: &[Arc<dyn ScheduledTask>],
    ) -> AppResult<()> {
        for task in builtin_tasks {
            let name = task.name();
            match self.job_repo.find_by_name(db, name).await? {
                Some(job_record) => {
                    // 数据库中有配置 → 使用 DB 的 cron 和 status 注册
                    let paused = job_record.status == job::Model::STATUS_PAUSED;
                    self.scheduler
                        .register(task.clone(), Some(&job_record.cron_expr), paused)
                        .await?;
                }
                None => {
                    // 数据库中没有 → 插入默认记录 + 使用默认 cron 注册
                    let mut entity = job::Model {
                        id: snowflake::next_snowflake_id(),
                        name: name.to_string(),
                        group_name: "system".into(),
                        cron_expr: task.cron().to_string(),
                        misfire_policy: "1".to_string(),
                        concurrent: "0".to_string(),
                        status: job::Model::STATUS_NORMAL.to_string(),
                        remark: Some(task.description().to_string()),
                        create_time: Default::default(),
                        update_time: Default::default(),
                    };
                    entity.fill_on_insert(&FillContext::new());
                    self.job_repo.insert(db, entity).await?;
                    // 使用任务默认 cron 注册
                    self.scheduler.register(task.clone(), None, false).await?;
                }
            }
        }

        Ok(())
    }

    /// 列出全部任务（合并 DB 状态 + scheduler 内存状态，包含暂停任务）
    pub async fn list_all(&self, db: &DatabaseConnection) -> AppResult<Vec<JobVo>> {
        let db_jobs = self.job_repo.find_all(db).await?;
        let scheduler_tasks = self.scheduler.list().await;

        let mut vos: Vec<JobVo> = db_jobs
            .into_iter()
            .map(|j| {
                let sched_info = scheduler_tasks.iter().find(|t| t.name == j.name);
                JobVo {
                    id: j.id.to_string(),
                    name: j.name,
                    group_name: j.group_name,
                    cron_expr: j.cron_expr,
                    misfire_policy: j.misfire_policy,
                    concurrent: j.concurrent,
                    status: j.status,
                    description: sched_info
                        .map(|t| t.description.clone())
                        .unwrap_or_default(),
                    next_fire_time: sched_info.and_then(|t| t.next_fire_time.clone()),
                    remark: j.remark,
                    create_time: j.create_time,
                    update_time: j.update_time,
                }
            })
            .collect();

        vos.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(vos)
    }

    /// 带过滤和 DB 分页的列表查询
    pub async fn list_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        name: Option<&str>,
        group_name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<JobVo>> {
        let result = self
            .job_repo
            .find_by_page_filtered(db, &query, name, group_name, status)
            .await?;
        let scheduler_tasks = self.scheduler.list().await;

        let vos: Vec<JobVo> = result
            .records
            .into_iter()
            .map(|j| {
                let sched_info = scheduler_tasks.iter().find(|t| t.name == j.name);
                JobVo {
                    id: j.id.to_string(),
                    name: j.name,
                    group_name: j.group_name,
                    cron_expr: j.cron_expr,
                    misfire_policy: j.misfire_policy,
                    concurrent: j.concurrent,
                    status: j.status,
                    description: sched_info
                        .map(|t| t.description.clone())
                        .unwrap_or_default(),
                    next_fire_time: sched_info.and_then(|t| t.next_fire_time.clone()),
                    remark: j.remark,
                    create_time: j.create_time,
                    update_time: j.update_time,
                }
            })
            .collect();

        Ok(PageResult {
            records: vos,
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }

    /// 新建定时任务
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        db: &DatabaseConnection,
        name: &str,
        cron_expr: &str,
        group_name: Option<&str>,
        misfire_policy: Option<&str>,
        concurrent: Option<&str>,
        remark: Option<&str>,
    ) -> AppResult<JobVo> {
        // 验证 cron 表达式
        if cron::Schedule::from_str(cron_expr).is_err() {
            return Err(AppError::Validation(format!(
                "无效的 cron 表达式: {}",
                cron_expr
            )));
        }

        // 检查任务名是否已存在
        if self.job_repo.find_by_name(db, name).await?.is_some() {
            return Err(AppError::Conflict(format!("任务名称 '{}' 已存在", name)));
        }

        let mut entity = job::Model {
            id: snowflake::next_snowflake_id(),
            name: name.to_string(),
            group_name: group_name.unwrap_or("default").to_string(),
            cron_expr: cron_expr.to_string(),
            misfire_policy: misfire_policy.unwrap_or("1").to_string(),
            concurrent: concurrent.unwrap_or("0").to_string(),
            status: job::Model::STATUS_NORMAL.to_string(),
            remark: remark.map(|s| s.to_string()),
            create_time: Default::default(),
            update_time: Default::default(),
        };
        entity.fill_on_insert(&FillContext::new());

        let saved = self.job_repo.insert(db, entity).await?;

        // 注册到调度器（仅当有对应的内置任务实现时）
        // 对于用户自定义任务，这里只创建 DB 记录，不注册到调度器

        Ok(JobVo {
            id: saved.id.to_string(),
            name: saved.name,
            group_name: saved.group_name,
            cron_expr: saved.cron_expr,
            misfire_policy: saved.misfire_policy,
            concurrent: saved.concurrent,
            status: saved.status,
            description: saved.remark.clone().unwrap_or_default(),
            next_fire_time: None,
            remark: saved.remark,
            create_time: saved.create_time,
            update_time: saved.update_time,
        })
    }

    /// 删除定时任务
    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let entity = self
            .job_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("任务不存在".into()))?;

        // 从调度器注销
        let _ = self.scheduler.unregister(&entity.name).await;

        // 删除 DB 记录
        self.job_repo.delete(db, id).await
    }

    /// 更新 cron / status / remark / misfire_policy / concurrent
    ///
    /// 执行顺序：校验 cron → 同步调度器 → 持久化 DB，
    /// 确保不会因 cron 无效或调度器操作失败导致 DB 和内存不一致。
    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        cron_expr: Option<String>,
        status: Option<String>,
        remark: Option<String>,
        misfire_policy: Option<String>,
        concurrent: Option<String>,
    ) -> AppResult<()> {
        let entity = self
            .job_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| ryframe_common::AppError::NotFound("任务不存在".into()))?;

        // 校验 cron 表达式
        if let Some(ref cron) = cron_expr {
            if cron::Schedule::from_str(cron).is_err() {
                return Err(AppError::Validation(format!(
                    "无效的 cron 表达式: {}",
                    cron
                )));
            }
        }

        // 先同步状态到调度器（确保调度器操作不会失败后再写 DB）
        if let Some(ref s) = status {
            if *s == job::Model::STATUS_PAUSED {
                self.scheduler.pause(&entity.name).await?;
            } else {
                self.scheduler.resume(&entity.name).await?;
            }
        }

        // 同步 cron 到调度器
        if let Some(ref cron) = cron_expr {
            self.scheduler.update_cron(&entity.name, cron).await?;
        }

        // 调度器同步成功后，持久化到 DB
        self.job_repo
            .update_cron(db, id, cron_expr, status, remark, misfire_policy, concurrent)
            .await
    }

    /// 暂停
    pub async fn pause(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let entity = self
            .job_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| ryframe_common::AppError::NotFound("任务不存在".into()))?;

        // 先同步调度器，确保操作不会失败后再写 DB
        self.scheduler.pause(&entity.name).await?;

        self.job_repo
            .update_status(db, id, job::Model::STATUS_PAUSED.to_string())
            .await
    }

    /// 恢复
    pub async fn resume(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let entity = self
            .job_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| ryframe_common::AppError::NotFound("任务不存在".into()))?;

        // 先同步调度器，确保操作不会失败后再写 DB
        self.scheduler.resume(&entity.name).await?;

        self.job_repo
            .update_status(db, id, job::Model::STATUS_NORMAL.to_string())
            .await
    }

    /// 立即触发一次
    pub async fn trigger_once(&self, name: &str) -> AppResult<TaskHistory> {
        self.scheduler.trigger_once(name).await
    }

    /// 根据 ID 触发一次（含 ID→name 查找）
    pub async fn trigger_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<TaskHistory> {
        let entity = self
            .job_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("任务不存在".into()))?;
        self.scheduler.trigger_once(&entity.name).await
    }

    /// 清空所有执行日志
    pub async fn clean_logs(&self, db: &DatabaseConnection) -> AppResult<u64> {
        self.job_log_repo.clean_all(db).await
    }

    /// 执行历史分页查询
    pub async fn log_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        job_name: Option<&str>,
        status: Option<String>,
        begin: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    ) -> AppResult<PageResult<JobLogVo>> {
        let result = self
            .job_log_repo
            .find_by_page_filtered(db, query, job_name, status, begin, end)
            .await?;

        let vos = result
            .records
            .into_iter()
            .map(|m| JobLogVo {
                id: m.id.to_string(),
                job_name: m.job_name,
                job_group: m.job_group,
                message: m.message,
                status: m.status,
                error_msg: m.error_msg,
                cost_ms: m.cost_ms,
                start_time: m.start_time,
            })
            .collect();

        Ok(PageResult {
            records: vos,
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }
}
