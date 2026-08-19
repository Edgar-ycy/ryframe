use std::sync::Arc;

use ryframe_db::{ControlDatabaseCluster, MessageRepository, OutboxEventRepository};
use ryframe_kernel::{AppError, AppResult};

use crate::JobQueue;

mod inbox;
mod publish;
mod retention;
mod types;

pub use types::*;

/// 消息投递任务的稳定类型标识。
pub const MESSAGE_DISPATCH_JOB_TYPE: &str = "system.message.dispatch";
/// 供 API 实例订阅的跨实例消息唤醒频道。
pub const MESSAGE_DISPATCH_REDIS_CHANNEL: &str = "ryframe:message:dispatch";
/// 每日清理过期消息的稳定任务类型标识。
pub const MESSAGE_RETENTION_JOB_TYPE: &str = "system.message.retention";

/// MySQL 持久化消息中心服务。
pub struct MessageService {
    db: ControlDatabaseCluster,
    repository: MessageRepository,
    outbox: OutboxEventRepository,
    queue: Arc<JobQueue>,
    config: crate::MessagingPolicy,
}

impl MessageService {
    /// 使用主库和持久化任务队列构造服务。
    pub fn new(
        db: ControlDatabaseCluster,
        queue: Arc<JobQueue>,
        config: crate::MessagingPolicy,
    ) -> Self {
        Self {
            db,
            repository: MessageRepository,
            outbox: OutboxEventRepository,
            queue,
            config,
        }
    }

    pub(super) fn ensure_enabled(&self) -> AppResult<()> {
        if self.config.enabled {
            Ok(())
        } else {
            Err(AppError::ServiceUnavailable("消息中心已关闭".into()))
        }
    }
}

pub(super) fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
