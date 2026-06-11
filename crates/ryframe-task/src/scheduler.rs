use std::{collections::HashMap, str::FromStr, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use cron::Schedule;
use ryframe_core::{DistributedLock, LockGuard};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::{
    task_history::{TaskHistory, TaskHistoryPersister, TaskHistoryStore},
    task_manager::{ScheduledTask, TaskContext},
};

/// 分布式锁默认 TTL（5 分钟），防止持有者崩溃导致死锁
const DEFAULT_LOCK_TTL_SECS: u64 = 300;

#[derive(Debug, Clone, Serialize)]
pub struct TaskInfo {
    pub name: String,
    pub cron: String,
    pub description: String,
    pub paused: bool,
    pub next_fire_time: Option<String>,
}

pub struct TaskScheduler {
    tasks: Arc<RwLock<HashMap<String, RegisteredTask>>>,
    history: TaskHistoryStore,
    ctx: TaskContext,
    /// 分布式锁（多实例防重复执行，None 表示单实例模式）
    distributed_lock: Option<Arc<dyn DistributedLock>>,
    /// 优雅关闭通知
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    /// 日志持久化回调（写入 DB 等外部存储）
    persister: Option<Arc<dyn TaskHistoryPersister>>,
}

struct RegisteredTask {
    task: Arc<dyn ScheduledTask>,
    schedule: Schedule,
    cron_expr: String,
    paused: bool,
    last_fired_at: Option<DateTime<Utc>>,
}

impl TaskScheduler {
    /// 创建调度器（单实例模式，不使用分布式锁）
    pub fn new(ctx: TaskContext) -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            history: TaskHistoryStore::new(500),
            ctx,
            distributed_lock: None,
            shutdown_tx,
            shutdown_rx,
            persister: None,
        }
    }

    /// 创建带分布式锁的调度器（多实例防重复执行）
    pub fn with_distributed_lock(
        ctx: TaskContext,
        distributed_lock: Arc<dyn DistributedLock>,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            history: TaskHistoryStore::new(500),
            ctx,
            distributed_lock: Some(distributed_lock),
            shutdown_tx,
            shutdown_rx,
            persister: None,
        }
    }

    /// 设置日志持久化回调（如写入 DB）
    pub fn set_persister(&mut self, persister: Arc<dyn TaskHistoryPersister>) {
        self.persister = Some(persister);
    }

    /// 发送优雅关闭信号
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        tracing::info!("TaskScheduler 收到关闭信号");
    }

    /// 注册一个定时任务
    ///
    /// - `task`: 任务实例
    /// - `cron_override`: 如果从数据库加载了自定义 cron，则覆盖任务默认值
    /// - `paused`: 初始暂停状态（从数据库加载）
    pub async fn register(
        &self,
        task: Arc<dyn ScheduledTask>,
        cron_override: Option<&str>,
        paused: bool,
    ) -> ryframe_common::AppResult<()> {
        let cron_str = cron_override.unwrap_or_else(|| task.cron()).to_string();
        let schedule = Schedule::from_str(&cron_str).map_err(|e| {
            ryframe_common::AppError::Config(format!("无效的 cron 表达式 '{}': {}", cron_str, e))
        })?;

        let mut tasks = self.tasks.write().await;
        let task_name = task.name().to_string();
        tracing::info!(
            "已注册定时任务: {} (cron={}, paused={})",
            task_name,
            cron_str,
            paused
        );
        tasks.insert(
            task_name,
            RegisteredTask {
                task,
                schedule,
                cron_expr: cron_str,
                paused,
                last_fired_at: None,
            },
        );
        Ok(())
    }

    pub async fn pause(&self, name: &str) -> ryframe_common::AppResult<()> {
        let mut tasks = self.tasks.write().await;
        match tasks.get_mut(name) {
            Some(rt) => {
                rt.paused = true;
                tracing::info!("已暂停定时任务: {}", name);
                Ok(())
            }
            None => Err(ryframe_common::AppError::NotFound(format!(
                "任务不存在: {}",
                name
            ))),
        }
    }

    pub async fn resume(&self, name: &str) -> ryframe_common::AppResult<()> {
        let mut tasks = self.tasks.write().await;
        match tasks.get_mut(name) {
            Some(rt) => {
                rt.paused = false;
                tracing::info!("已恢复定时任务: {}", name);
                Ok(())
            }
            None => Err(ryframe_common::AppError::NotFound(format!(
                "任务不存在: {}",
                name
            ))),
        }
    }

    /// 更新任务的 cron 表达式
    pub async fn update_cron(&self, name: &str, new_cron: &str) -> ryframe_common::AppResult<()> {
        let schedule = Schedule::from_str(new_cron).map_err(|e| {
            ryframe_common::AppError::Config(format!("无效的 cron 表达式 '{}': {}", new_cron, e))
        })?;
        let mut tasks = self.tasks.write().await;
        match tasks.get_mut(name) {
            Some(rt) => {
                rt.schedule = schedule;
                rt.cron_expr = new_cron.to_string();
                tracing::info!("已更新定时任务 cron: {} -> {}", name, new_cron);
                Ok(())
            }
            None => Err(ryframe_common::AppError::NotFound(format!(
                "任务不存在: {}",
                name
            ))),
        }
    }

    /// 注销任务（从调度器中移除）
    pub async fn unregister(&self, name: &str) -> ryframe_common::AppResult<()> {
        let mut tasks = self.tasks.write().await;
        if tasks.remove(name).is_some() {
            tracing::info!("已注销定时任务: {}", name);
            Ok(())
        } else {
            Err(ryframe_common::AppError::NotFound(format!(
                "任务不存在: {}",
                name
            )))
        }
    }

    /// 检查任务是否已注册
    pub async fn is_registered(&self, name: &str) -> bool {
        let tasks = self.tasks.read().await;
        tasks.contains_key(name)
    }

    pub async fn list(&self) -> Vec<TaskInfo> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .map(|(name, rt)| {
                let next = rt.schedule.upcoming(Utc).next().map(|t| t.to_rfc3339());
                TaskInfo {
                    name: name.clone(),
                    cron: rt.cron_expr.clone(),
                    description: rt.task.description().to_string(),
                    paused: rt.paused,
                    next_fire_time: next,
                }
            })
            .collect()
    }

    /// 尝试获取任务分布式锁，返回 LockGuard 在任务执行期间持有
    async fn acquire_lock(&self, task_name: &str) -> Option<LockGuard> {
        let lock = self.distributed_lock.as_ref()?;
        match lock
            .try_acquire(
                &format!("task:lock:{}", task_name),
                Duration::from_secs(DEFAULT_LOCK_TTL_SECS),
            )
            .await
        {
            Ok(guard) => {
                if guard.is_none() {
                    tracing::debug!("分布式锁被其他实例持有 [task={}], 跳过", task_name);
                }
                guard
            }
            Err(e) => {
                tracing::warn!("分布式锁异常 [task={}]: {}, 降级执行", task_name, e);
                None
            }
        }
    }

    pub async fn trigger_once(&self, name: &str) -> ryframe_common::AppResult<TaskHistory> {
        let task = {
            let tasks = self.tasks.read().await;
            tasks.get(name).map(|rt| rt.task.clone()).ok_or_else(|| {
                ryframe_common::AppError::NotFound(format!("任务不存在: {}", name))
            })?
        };

        // 分布式锁：防止多实例同时手动触发同一任务
        let _lock_guard = self.acquire_lock(name).await;

        let started_at = Utc::now();
        let ctx = self.ctx.clone();
        let result = task.execute(&ctx).await;
        let finished_at = Utc::now();
        let cost_ms = (finished_at - started_at).num_milliseconds();

        let history = match result {
            Ok(msg) => TaskHistory {
                task_name: name.to_string(),
                started_at,
                finished_at,
                cost_ms,
                status: TaskHistory::STATUS_SUCCESS.to_string(),
                message: msg,
            },
            Err(e) => TaskHistory {
                task_name: name.to_string(),
                started_at,
                finished_at,
                cost_ms,
                status: TaskHistory::STATUS_FAIL.to_string(),
                message: e.to_string(),
            },
        };

        self.history.push(history.clone()).await;

        // 持久化到外部存储（如 DB）
        if let Some(ref persister) = self.persister {
            if let Err(e) = persister.persist(&history).await {
                tracing::warn!("持久化任务执行历史失败 [task={}]: {}", name, e);
            }
        }

        Ok(history)
    }

    pub fn history(&self) -> TaskHistoryStore {
        self.history.clone_store()
    }

    /// 启动主循环（spawn 后台 task），每秒 tick 检查到期任务
    ///
    /// 支持优雅关闭：收到 shutdown 信号后停止调度新任务。
    pub fn spawn(self: Arc<Self>) {
        let mut shutdown_rx = self.shutdown_rx.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        self.run_due_tasks(Utc::now()).await;
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::info!("TaskScheduler 已停止调度");
                            break;
                        }
                    }
                }
            }
        });
    }

    /// 检查并执行到期的任务
    async fn run_due_tasks(&self, now: DateTime<Utc>) {
        let due_tasks: Vec<Arc<dyn ScheduledTask>> = {
            let mut tasks = self.tasks.write().await;
            tasks
                .iter_mut()
                .filter(|(_, rt)| !rt.paused)
                .filter_map(|(_, rt)| {
                    let next = rt.schedule.upcoming(Utc).next();
                    if let Some(next_time) = next
                        && next_time <= now + chrono::Duration::seconds(1)
                        && rt.last_fired_at.map(|t| t < next_time).unwrap_or(true)
                    {
                        rt.last_fired_at = Some(now);
                        return Some(rt.task.clone());
                    }
                    None
                })
                .collect()
        };

        for task in due_tasks {
            // 分布式锁：防止多实例 cron 调度同时执行同一任务
            let name = task.name().to_string();
            let _lock_guard = self.acquire_lock(&name).await;

            let started_at = Utc::now();
            let ctx = self.ctx.clone();
            let result = task.execute(&ctx).await;
            let finished_at = Utc::now();
            let cost_ms = (finished_at - started_at).num_milliseconds();

            let history = match result {
                Ok(msg) => TaskHistory {
                    task_name: name,
                    started_at,
                    finished_at,
                    cost_ms,
                    status: TaskHistory::STATUS_SUCCESS.to_string(),
                    message: msg,
                },
                Err(e) => TaskHistory {
                    task_name: name,
                    started_at,
                    finished_at,
                    cost_ms,
                    status: TaskHistory::STATUS_FAIL.to_string(),
                    message: e.to_string(),
                },
            };

            self.history.push(history.clone()).await;

            // 持久化到外部存储（如 DB）
            if let Some(ref persister) = self.persister {
                if let Err(e) = persister.persist(&history).await {
                    tracing::warn!(
                        "持久化任务执行历史失败 [task={}]: {}",
                        task.name(),
                        e
                    );
                }
            }
        }
    }
}
