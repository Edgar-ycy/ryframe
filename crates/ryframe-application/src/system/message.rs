use std::sync::Arc;

use ryframe_kernel::{AppError, AppResult};

use crate::{JobQueue, ports::system::MessagePersistencePort};

mod inbox;
mod publish;
mod retention;
mod types;

pub use publish::validate_message_text_pair;
pub use types::*;

/// 消息投递任务的稳定类型标识。
pub const MESSAGE_DISPATCH_JOB_TYPE: &str = "system.message.dispatch";
/// 供 API 实例订阅的跨实例消息唤醒频道。
pub const MESSAGE_DISPATCH_REDIS_CHANNEL: &str = "ryframe:message:dispatch";
/// 每日清理过期消息的稳定任务类型标识。
pub const MESSAGE_RETENTION_JOB_TYPE: &str = "system.message.retention";

/// MySQL 持久化消息中心服务。
pub struct MessageService {
    persistence: Arc<dyn MessagePersistencePort>,
    queue: Arc<JobQueue>,
    config: crate::MessagingPolicy,
}

impl MessageService {
    /// 使用主库和持久化任务队列构造服务。
    pub fn new(
        persistence: Arc<dyn MessagePersistencePort>,
        queue: Arc<JobQueue>,
        config: crate::MessagingPolicy,
    ) -> Self {
        Self {
            persistence,
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
