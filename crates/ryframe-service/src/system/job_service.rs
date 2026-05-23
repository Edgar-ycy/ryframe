use chrono::{DateTime, Utc};
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use ryframe_db::entities::job;
use ryframe_db::{JobLogRepository, JobRepository};
use ryframe_task::{ScheduledTask, TaskHistory, TaskScheduler};
use sea_orm::DatabaseConnection;
use serde::Serialize;
use ryframe_common::utils::snowflake;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct JobVo {
    pub id: i64,
    pub name: String,
    pub group_name: String,
    pub cron_expr: String,
    pub status: String,
    pub description: String,
    pub next_fire_time: Option<String>,
    pub remark: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JobLogVo {
    pub id: i64,
    pub job_name: String,
    pub job_group: String,
    pub message: String,
    pub status: String,
    pub error_msg: Option<String>,
    pub cost_ms: i64,
    pub start_time: DateTime<Utc>,
}



pub struct JobServiceImpl {
    pub job_repo: JobRepository,
    pub job_log_repo: JobLogRepository,
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
                    let entity = job::Model {
                        id: snowflake::next_snowflake_id(),
                        name: name.to_string(),
                        group_name: "system".into(),
                        cron_expr: task.cron().to_string(),
                        misfire_policy: "1".to_string(),
                        concurrent: "0".to_string(),
                        status: job::Model::STATUS_NORMAL.to_string(),
                        remark: Some(task.description().to_string()),
                        create_time: Utc::now(),
                        update_time: Utc::now(),
                    };
                    self.job_repo.insert(db, entity).await?;
                    // 使用任务默认 cron 注册
                    self.scheduler.register(task.clone(), None, false).await?;
                }
            }
        }

        Ok(())
    }

    /// 列出全部任务（合并 DB 状态 + scheduler 内存状态）
    pub async fn list_all(&self, db: &DatabaseConnection) -> AppResult<Vec<JobVo>> {
        let db_jobs = self.job_repo.find_all_enabled(db).await?;
        let scheduler_tasks = self.scheduler.list().await;

        let mut vos: Vec<JobVo> = db_jobs
            .into_iter()
            .map(|j| {
                let sched_info = scheduler_tasks.iter().find(|t| t.name == j.name);
                JobVo {
                    id: j.id,
                    name: j.name,
                    group_name: j.group_name,
                    cron_expr: j.cron_expr,
                    status: j.status,
                    description: sched_info
                        .map(|t| t.description.clone())
                        .unwrap_or_default(),
                    next_fire_time: sched_info.and_then(|t| t.next_fire_time.clone()),
                    remark: j.remark,
                }
            })
            .collect();

        vos.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(vos)
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
            return Err(AppError::Validation(format!("无效的 cron 表达式: {}", cron_expr)));
        }

        // 检查任务名是否已存在
        if self.job_repo.find_by_name(db, name).await?.is_some() {
            return Err(AppError::Conflict(format!("任务名称 '{}' 已存在", name)));
        }

        let now = Utc::now();
        let entity = job::Model {
            id: snowflake::next_snowflake_id(),
            name: name.to_string(),
            group_name: group_name.unwrap_or("default").to_string(),
            cron_expr: cron_expr.to_string(),
            misfire_policy: misfire_policy.unwrap_or("1").to_string(),
            concurrent: concurrent.unwrap_or("0").to_string(),
            status: job::Model::STATUS_NORMAL.to_string(),
            remark: remark.map(|s| s.to_string()),
            create_time: now,
            update_time: now,
        };

        let saved = self.job_repo.insert(db, entity).await?;

        // 注册到调度器（仅当有对应的内置任务实现时）
        // 对于用户自定义任务，这里只创建 DB 记录，不注册到调度器

        Ok(JobVo {
            id: saved.id,
            name: saved.name,
            group_name: saved.group_name,
            cron_expr: saved.cron_expr,
            status: saved.status,
            description: saved.remark.clone().unwrap_or_default(),
            next_fire_time: None,
            remark: saved.remark,
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

    /// 更新 cron / status / remark
    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        cron_expr: Option<String>,
        status: Option<String>,
        remark: Option<String>,
    ) -> AppResult<()> {
        let entity = self
            .job_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| ryframe_common::AppError::NotFound("任务不存在".into()))?;

        // 持久化到 DB
        self.job_repo
            .update_cron(db, id, cron_expr.clone(), status.clone(), remark)
            .await?;

        // 同步状态到调度器
        if let Some(ref status) = status {
            if *status == job::Model::STATUS_PAUSED {
                let _ = self.scheduler.pause(&entity.name).await;
            } else {
                let _ = self.scheduler.resume(&entity.name).await;
            }
        }

        // 同步 cron 到调度器
        if let Some(ref cron) = cron_expr {
            let _ = self.scheduler.update_cron(&entity.name, cron).await;
        }

        Ok(())
    }

    /// 暂停
    pub async fn pause(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let entity = self
            .job_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| ryframe_common::AppError::NotFound("任务不存在".into()))?;

        self.job_repo
            .update_status(db, id, job::Model::STATUS_PAUSED.to_string())
            .await?;

        self.scheduler.pause(&entity.name).await
    }

    /// 恢复
    pub async fn resume(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let entity = self
            .job_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| ryframe_common::AppError::NotFound("任务不存在".into()))?;

        self.job_repo
            .update_status(db, id, job::Model::STATUS_NORMAL.to_string())
            .await?;

        self.scheduler.resume(&entity.name).await
    }

    /// 立即触发一次
    pub async fn trigger_once(&self, name: &str) -> AppResult<TaskHistory> {
        self.scheduler.trigger_once(name).await
    }

    /// 执行历史分页查询
    pub async fn log_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        job_name: Option<&str>,
        status: Option<String>,
    ) -> AppResult<PageResult<JobLogVo>> {
        let begin: Option<DateTime<Utc>> = None;
        let end: Option<DateTime<Utc>> = None;
        let result = self
            .job_log_repo
            .find_by_page_filtered(db, query, job_name, status, begin, end)
            .await?;

        let vos = result
            .records
            .into_iter()
            .map(|m| JobLogVo {
                id: m.id,
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

