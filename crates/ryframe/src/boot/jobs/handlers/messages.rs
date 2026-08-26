use std::sync::Arc;

use async_trait::async_trait;
use ryframe_application::{
    JobHandler, MessageDispatchJobHandler, MessageRetentionJobHandler, MessageWakeupPublisher,
};

use super::super::JobWorkerDependencies;

struct RedisMessageWakeupPublisher {
    client: ryframe_adapters::RedisClient,
}

#[async_trait]
impl MessageWakeupPublisher for RedisMessageWakeupPublisher {
    async fn publish(&self, message_id: i64) -> Result<(), String> {
        self.client
            .publish(
                ryframe_application::system::operations::MESSAGE_DISPATCH_REDIS_CHANNEL,
                message_id.to_string(),
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub(super) fn handlers(dependencies: &JobWorkerDependencies) -> Vec<Arc<dyn JobHandler>> {
    if !dependencies.messaging_enabled {
        return Vec::new();
    }
    let wakeup = dependencies.redis.clone().map(|client| {
        Arc::new(RedisMessageWakeupPublisher { client }) as Arc<dyn MessageWakeupPublisher>
    });
    vec![
        Arc::new(
            MessageDispatchJobHandler::new(dependencies.message.clone(), wakeup)
                .with_redis_wakeup_failure_observer(Arc::new(|| {
                    ryframe_adapters::metrics::record_redis_degraded("message_dispatch_wakeup");
                })),
        ),
        Arc::new(
            MessageRetentionJobHandler::new(dependencies.message.clone()).with_deleted_observer(
                Arc::new(ryframe_adapters::metrics::record_message_retention_deleted),
            ),
        ),
    ]
}
