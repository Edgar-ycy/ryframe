use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use ryframe_adapters::RedisClient;
use ryframe_db::background_job;
use ryframe_kernel::{AppError, AppResult};
use serde::Deserialize;

use super::worker::JobHandler;
use crate::system::{
    EXPORT_CLEANUP_JOB_TYPE, EXPORT_JOB_TYPE, ExportService, MESSAGE_DISPATCH_JOB_TYPE,
    MESSAGE_DISPATCH_REDIS_CHANNEL, MESSAGE_RETENTION_JOB_TYPE, MessageService,
};

type RedisWakeupFailureCallback = dyn Fn() + Send + Sync;

/// 执行对象存储导出并更新公开导出任务状态的处理器。
pub struct ExportJobHandler {
    service: Arc<ExportService>,
}

impl ExportJobHandler {
    pub fn new(service: Arc<ExportService>) -> Self {
        Self { service }
    }
}

fn is_terminal_export_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Validation(_)
            | AppError::Authentication(_)
            | AppError::Authorization(_)
            | AppError::NotFound(_)
            | AppError::Conflict(_)
            | AppError::PayloadTooLarge(_)
    )
}

#[async_trait]
impl JobHandler for ExportJobHandler {
    fn job_type(&self) -> &'static str {
        EXPORT_JOB_TYPE
    }

    async fn handle(&self, job: &background_job::Model) -> AppResult<()> {
        self.service.execute_background_job(job.id).await
    }

    fn should_dead_letter(&self, error: &AppError) -> bool {
        is_terminal_export_error(error)
    }
}

/// 清理过期导出文件的处理器。
pub struct ExportCleanupJobHandler {
    service: Arc<ExportService>,
}

impl ExportCleanupJobHandler {
    pub fn new(service: Arc<ExportService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl JobHandler for ExportCleanupJobHandler {
    fn job_type(&self) -> &'static str {
        EXPORT_CLEANUP_JOB_TYPE
    }

    async fn handle(&self, _job: &background_job::Model) -> AppResult<()> {
        let cleaned = self.service.cleanup_expired().await?;
        if cleaned == 0 {
            tracing::debug!("没有需要清理的过期导出结果");
        } else {
            tracing::info!(cleaned, "已清理过期导出结果");
        }
        Ok(())
    }
}

/// 消息投递任务的序列化载荷。
#[derive(Debug, Deserialize)]
struct MessageDispatchJobPayload {
    message_id: String,
}

/// 将消息投递任务交给消息中心服务处理。
pub struct MessageDispatchJobHandler {
    service: Arc<MessageService>,
    redis: Option<RedisClient>,
    on_redis_wakeup_failure: Arc<RedisWakeupFailureCallback>,
    redis_wakeup_degraded: AtomicBool,
}

/// 清理到期消息及其级联收件箱记录的任务处理器。
pub struct MessageRetentionJobHandler {
    service: Arc<MessageService>,
    on_deleted: Arc<dyn Fn(u64) + Send + Sync>,
}

impl MessageRetentionJobHandler {
    /// 使用消息中心服务创建过期清理处理器。
    pub fn new(service: Arc<MessageService>) -> Self {
        Self {
            service,
            on_deleted: Arc::new(|_| {}),
        }
    }

    /// 注入删除计数观察器，使传输层可记录指标而不反向依赖中间件 crate。
    pub fn with_deleted_observer(mut self, observer: Arc<dyn Fn(u64) + Send + Sync>) -> Self {
        self.on_deleted = observer;
        self
    }
}

#[async_trait]
impl JobHandler for MessageRetentionJobHandler {
    fn job_type(&self) -> &'static str {
        MESSAGE_RETENTION_JOB_TYPE
    }

    async fn handle(&self, _job: &background_job::Model) -> AppResult<()> {
        let deleted = self.service.delete_expired().await?;
        (self.on_deleted)(deleted);
        if deleted == 0 {
            tracing::debug!("没有需要清理的过期消息");
        } else {
            tracing::info!(deleted, "已完成过期消息清理");
        }
        Ok(())
    }
}

impl MessageDispatchJobHandler {
    /// 使用消息中心服务和可选 Redis 唤醒通道创建处理器。
    ///
    /// Redis 只用于降低在线投递延迟；未配置 Redis 时，客户端仍会通过收件箱补拉消息。
    pub fn new(service: Arc<MessageService>, redis: Option<RedisClient>) -> Self {
        Self {
            service,
            redis,
            on_redis_wakeup_failure: Arc::new(|| {}),
            redis_wakeup_degraded: AtomicBool::new(false),
        }
    }

    /// 注入 Redis 唤醒失败观察器，使组合根能够记录运行时降级指标。
    pub fn with_redis_wakeup_failure_observer(
        mut self,
        observer: Arc<RedisWakeupFailureCallback>,
    ) -> Self {
        self.on_redis_wakeup_failure = observer;
        self
    }

    fn report_redis_wakeup_success(&self) {
        if self.redis_wakeup_degraded.swap(false, Ordering::AcqRel) {
            tracing::info!("消息 Redis 唤醒已恢复");
        }
    }

    fn report_redis_wakeup_failure(&self, error: impl std::fmt::Display) {
        (self.on_redis_wakeup_failure)();
        if !self.redis_wakeup_degraded.swap(true, Ordering::AcqRel) {
            tracing::warn!(%error, "消息 Redis 唤醒失败，客户端将通过收件箱补拉");
        } else {
            tracing::debug!(%error, "消息 Redis 唤醒仍不可用");
        }
    }
}

#[async_trait]
impl JobHandler for MessageDispatchJobHandler {
    fn job_type(&self) -> &'static str {
        MESSAGE_DISPATCH_JOB_TYPE
    }

    async fn handle(&self, job: &background_job::Model) -> AppResult<()> {
        let payload: MessageDispatchJobPayload = serde_json::from_value(job.payload.clone())
            .map_err(|error| AppError::Validation(format!("消息投递任务载荷无效: {error}")))?;
        let message_id = payload
            .message_id
            .parse::<i64>()
            .map_err(|_| AppError::Validation("消息投递任务的 message_id 无效".into()))?;
        self.service.dispatch(message_id).await?;
        if let Some(redis) = &self.redis {
            match redis
                .publish(MESSAGE_DISPATCH_REDIS_CHANNEL, message_id.to_string())
                .await
            {
                Ok(_) => self.report_redis_wakeup_success(),
                Err(error) => self.report_redis_wakeup_failure(error),
            }
        }
        Ok(())
    }
}
