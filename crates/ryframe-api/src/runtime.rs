use std::sync::Arc;

use chrono::Utc;
use ryframe_core::{
    DistributedLock, Event, EventBus, MqBackend, RedisClient, TaskMessage, TaskQueue,
    create_distributed_lock, create_message_queue,
    feature_flag::{FeatureFlags, FeaturePresets},
    message_queue::publish_json,
    resilience::CircuitBreaker,
};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct UserImportCompletedEvent {
    pub tenant_id: String,
    pub operator: String,
    pub success_count: u64,
    pub fail_count: u64,
    pub occurred_at: String,
}

impl Event for UserImportCompletedEvent {}

#[derive(Debug, Clone, Serialize)]
pub struct FileUploadedEvent {
    pub tenant_id: String,
    pub operator: String,
    pub file_id: i64,
    pub file_url: String,
    pub bucket: String,
    pub occurred_at: String,
}

impl Event for FileUploadedEvent {}

#[derive(Clone)]
pub struct RuntimeComponents {
    pub event_bus: EventBus,
    pub message_queue: Arc<MqBackend>,
    pub feature_flags: FeatureFlags,
    pub distributed_lock: Arc<dyn DistributedLock>,
    pub task_queue: TaskQueue,
    pub upload_circuit_breaker: Arc<CircuitBreaker>,
}

impl RuntimeComponents {
    pub fn new(redis: Option<RedisClient>) -> Self {
        let feature_flags = FeaturePresets::standard()
            .with_flag("user_import", true, "User import workflow")
            .with_flag("file_upload", true, "Authenticated file upload workflow")
            .with_flag("business_events", true, "Business event publication")
            .with_flag("background_tasks", true, "Background task queue dispatch");

        Self {
            event_bus: EventBus::new(),
            message_queue: Arc::new(create_message_queue()),
            feature_flags,
            distributed_lock: create_distributed_lock(redis.as_ref()),
            task_queue: TaskQueue::new(redis, "ryframe_business"),
            upload_circuit_breaker: Arc::new(CircuitBreaker::default_config()),
        }
    }

    pub async fn emit_user_import_completed(&self, event: UserImportCompletedEvent) {
        if self.feature_flags.is_enabled_or("business_events", true) {
            self.event_bus.publish(event.clone()).await;
            if let Err(err) =
                publish_json(self.message_queue.as_ref(), "user.import.completed", &event).await
            {
                tracing::warn!("publish user import message failed: {}", err);
            }
        }

        if self.feature_flags.is_enabled_or("background_tasks", true) {
            self.enqueue_task("user_import_completed", &event).await;
        }
    }

    pub async fn emit_file_uploaded(&self, event: FileUploadedEvent) {
        if self.feature_flags.is_enabled_or("business_events", true) {
            self.event_bus.publish(event.clone()).await;
            if let Err(err) =
                publish_json(self.message_queue.as_ref(), "file.uploaded", &event).await
            {
                tracing::warn!("publish file upload message failed: {}", err);
            }
        }

        if self.feature_flags.is_enabled_or("background_tasks", true) {
            self.enqueue_task("file_uploaded", &event).await;
        }
    }

    async fn enqueue_task<T: Serialize>(&self, task_type: &str, payload: &T) {
        let payload = match serde_json::to_string(payload) {
            Ok(payload) => payload,
            Err(err) => {
                tracing::warn!("serialize task payload failed: {}", err);
                return;
            }
        };

        let task = TaskMessage {
            task_type: task_type.to_string(),
            payload,
            created_at: Utc::now().timestamp_millis(),
            max_retries: 3,
            retry_count: 0,
        };

        if let Err(err) = self.task_queue.enqueue(&task).await {
            tracing::warn!("enqueue business task failed: {}", err);
        }
    }
}
