mod common;

use ryframe_config::JobConfig;
use ryframe_db::{
    DatabaseCluster, RecordOutboxEvent,
    entities::{background_job, outbox_event},
};
use ryframe_service::{
    JobQueue, OutboxRunResult, OutboxWorker, jobs::MESSAGE_PUBLISHED_OUTBOX_EVENT_TYPE,
    system::MESSAGE_DISPATCH_JOB_TYPE,
};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Schema, TransactionTrait};
use serde_json::json;
use std::sync::Arc;

/// 事件投递与后台任务创建必须作为一个事务完成，避免消息发布后丢失异步处理。
#[tokio::test]
async fn outbox_worker_publishes_message_event_as_idempotent_background_job() {
    let database = common::setup_test_db().await;
    let schema = Schema::new(sea_orm::DatabaseBackend::MySql);
    database
        .execute(&schema.create_table_from_entity(background_job::Entity))
        .await
        .unwrap();

    let cluster = DatabaseCluster::single(database.connection().clone());
    let queue = Arc::new(JobQueue::new(cluster));
    let repository = ryframe_db::OutboxEventRepository;
    let now = repository
        .database_utc_now(database.connection())
        .await
        .unwrap();
    let transaction = database.connection().begin().await.unwrap();
    let event = repository
        .record_in_transaction(
            &transaction,
            RecordOutboxEvent {
                tenant_id: Some("system".into()),
                event_type: MESSAGE_PUBLISHED_OUTBOX_EVENT_TYPE.into(),
                aggregate_type: "message".into(),
                aggregate_id: "900000000000001".into(),
                payload: json!({ "message_id": "900000000000001" }),
                available_at: now - chrono::Duration::seconds(1),
                max_attempts: 3,
                dedupe_key: Some("message:900000000000001".into()),
                traceparent: None,
                tracestate: None,
            },
            now,
        )
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    let worker = OutboxWorker::new(queue, &JobConfig::default()).unwrap();
    assert_eq!(
        worker.run_once("outbox-test-worker").await.unwrap(),
        OutboxRunResult::Published
    );

    let delivered = outbox_event::Entity::find_by_id(event.id)
        .one(database.connection())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivered.status, outbox_event::Model::STATUS_PUBLISHED);
    assert!(delivered.published_at.is_some());
    let jobs = background_job::Entity::find()
        .filter(background_job::Column::JobType.eq(MESSAGE_DISPATCH_JOB_TYPE))
        .filter(background_job::Column::DedupeKey.eq("message:900000000000001"))
        .all(database.connection())
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].status, background_job::Model::STATUS_PENDING);
    assert_eq!(jobs[0].payload, json!({ "message_id": "900000000000001" }));
}
