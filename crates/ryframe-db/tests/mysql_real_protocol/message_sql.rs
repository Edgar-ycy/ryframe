use ryframe_application::{
    TransactionAuditMode,
    ports::system::{
        MessageAudienceRecord, MessageAudienceRecordKind, MessageInboxFilter, MessageOutboxRecord,
        MessagePersistencePort, MessageTransaction, PublishMessageRecord,
    },
};
use ryframe_db::{ControlDatabaseCluster, application_ports, migration};
use sea_orm::DatabaseConnection;
use time::{Duration, OffsetDateTime, macros::datetime};

use super::mysql::{execute, require_count, run_mysql_test};

const TENANT: &str = "tenant-message-sql";
const RECIPIENT: i64 = 101;
const PUBLISHED_AT: OffsetDateTime = datetime!(2026-11-26 00:34:05.123456789 +05:30);
const STORED_PUBLISHED_AT: OffsetDateTime = datetime!(2026-11-25 19:04:05.123456 UTC);
const STORED_EXPIRES_AT: OffsetDateTime = datetime!(2026-11-25 19:04:15.123456 UTC);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn message_sql_preserves_utc_microseconds_and_lifecycle_boundaries() {
    run_mysql_test("message-sql-utc", |database| async move {
        for statement in migration::control_ddl_statements() {
            execute(&database, statement).await?;
        }
        super::install_test_database_id_generator()?;
        execute(
            &database,
            "INSERT INTO sys_tenant(id,tenant_id,name,status) \
             VALUES (90030,'tenant-message-sql','Message SQL','enabled')",
        )
        .await?;
        execute(
            &database,
            "INSERT INTO sys_user (id, tenant_id, username, password_hash, nickname) \
             VALUES (101, 'tenant-message-sql', 'recipient', 'fixture-hash', 'Recipient')",
        )
        .await?;
        let messages =
            application_ports::system::message(ControlDatabaseCluster::single(database.clone()));

        let transaction = messages.begin().await.map_err(|error| error.to_string())?;
        publish(transaction.as_ref(), "rolled-back").await?;
        transaction
            .rollback()
            .await
            .map_err(|error| error.to_string())?;
        for table in [
            "sys_message",
            "sys_message_recipient",
            "sys_message_audience",
            "sys_outbox_event",
        ] {
            require_count(
                &database,
                &format!("SELECT COUNT(*) AS value FROM {table}"),
                0,
            )
            .await?;
        }

        let transaction = messages.begin().await.map_err(|error| error.to_string())?;
        let message_id = publish(transaction.as_ref(), "committed").await?;
        transaction
            .commit(TransactionAuditMode::Skip)
            .await
            .map_err(|error| error.to_string())?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_outbox_event WHERE tenant_id = 'tenant-message-sql'",
            1,
        )
        .await?;
        verify_initial_rows(&database, messages.as_ref(), message_id).await?;
        verify_recipient_updates(&database, messages.as_ref(), message_id).await?;
        verify_expiry(&database, messages.as_ref(), message_id).await
    })
    .await;
}

async fn publish(transaction: &dyn MessageTransaction, source_id: &str) -> Result<i64, String> {
    let published = transaction
        .publish(
            PublishMessageRecord {
                tenant_id: TENANT.into(),
                topic: "message-sql".into(),
                title_text: Some("Title".into()),
                body_text: Some("Body".into()),
                title_key: None,
                body_key: None,
                args_json: None,
                severity: "info".into(),
                payload_json: None,
                source_type: Some("message-sql".into()),
                source_id: Some(source_id.into()),
                created_by: RECIPIENT,
                published_at: PUBLISHED_AT,
                expires_at: PUBLISHED_AT + Duration::seconds(10),
                audiences: vec![MessageAudienceRecord {
                    kind: MessageAudienceRecordKind::User,
                    target_id: RECIPIENT,
                }],
            },
            10,
        )
        .await
        .map_err(|error| error.to_string())?;
    assert!(published.inserted);
    assert_eq!(published.recipient_count, 1);
    let message_id = published.message.id;
    transaction
        .record_outbox(MessageOutboxRecord {
            tenant_id: TENANT.into(),
            event_type: "system.message.published".into(),
            aggregate_id: message_id.to_string(),
            payload: serde_json::json!({"message_id": message_id.to_string()}),
            available_at: PUBLISHED_AT,
            traceparent: None,
            tracestate: None,
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(message_id)
}

async fn verify_initial_rows(
    database: &DatabaseConnection,
    messages: &dyn MessagePersistencePort,
    message_id: i64,
) -> Result<(), String> {
    require_count(
        database,
        "SELECT COUNT(*) AS value FROM sys_message WHERE tenant_id = 'tenant-message-sql' \
         AND published_at = '2026-11-25 19:04:05.123456' AND expires_at = '2026-11-25 19:04:15.123456' \
         AND created_at = published_at AND updated_at = published_at",
        1,
    )
    .await?;
    require_count(
        database,
        "SELECT COUNT(*) AS value FROM sys_message_recipient WHERE tenant_id = 'tenant-message-sql' \
         AND user_id = 101 AND created_at = '2026-11-25 19:04:05.123456' \
         AND enqueued_at IS NULL AND acked_at IS NULL AND read_at IS NULL AND deleted_at IS NULL",
        1,
    )
    .await?;
    let message = messages
        .find_message(message_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("已发布消息未能从数据库读回")?;
    assert_eq!(message.published_at, STORED_PUBLISHED_AT);
    assert!(message.published_at.offset().is_utc());
    assert_eq!(message.expires_at, Some(STORED_EXPIRES_AT));
    let recipients = messages
        .unacknowledged_recipients(message_id, Some(&[RECIPIENT]), PUBLISHED_AT)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(recipients.len(), 1);
    assert_eq!(recipients[0].user_id, RECIPIENT);
    assert!(recipients[0].acked_at.is_none() && recipients[0].read_at.is_none());
    Ok(())
}

async fn verify_recipient_updates(
    database: &DatabaseConnection,
    messages: &dyn MessagePersistencePort,
    message_id: i64,
) -> Result<(), String> {
    let ids = [message_id];
    let first_ack = PUBLISHED_AT + Duration::SECOND;
    let first_enqueue = PUBLISHED_AT + Duration::seconds(2);
    let first_read = PUBLISHED_AT + Duration::seconds(3);
    let retry = PUBLISHED_AT + Duration::seconds(4);
    let transaction = messages.begin().await.map_err(|error| error.to_string())?;
    assert_eq!(
        transaction
            .acknowledge(TENANT, RECIPIENT, &ids, first_ack)
            .await
            .map_err(|error| error.to_string())?,
        1
    );
    assert_eq!(
        transaction
            .acknowledge(TENANT, RECIPIENT, &ids, retry)
            .await
            .map_err(|error| error.to_string())?,
        0
    );
    assert_eq!(
        transaction
            .mark_enqueued(message_id, first_enqueue)
            .await
            .map_err(|error| error.to_string())?,
        1
    );
    assert_eq!(
        transaction
            .mark_enqueued(message_id, retry)
            .await
            .map_err(|error| error.to_string())?,
        0
    );
    assert!(
        transaction
            .mark_read(TENANT, RECIPIENT, message_id, first_read)
            .await
            .map_err(|error| error.to_string())?
    );
    assert!(
        transaction
            .mark_read(TENANT, RECIPIENT, message_id, retry)
            .await
            .map_err(|error| error.to_string())?
    );
    transaction
        .commit(TransactionAuditMode::Skip)
        .await
        .map_err(|error| error.to_string())?;
    require_count(
        database,
        "SELECT COUNT(*) AS value FROM sys_message_recipient WHERE tenant_id = 'tenant-message-sql' \
         AND user_id = 101 AND acked_at = '2026-11-25 19:04:06.123456' \
         AND enqueued_at = '2026-11-25 19:04:07.123456' AND read_at = '2026-11-25 19:04:08.123456'",
        1,
    )
    .await
}

async fn verify_expiry(
    database: &DatabaseConnection,
    messages: &dyn MessagePersistencePort,
    message_id: i64,
) -> Result<(), String> {
    let expires_at = STORED_EXPIRES_AT.to_offset(PUBLISHED_AT.offset());
    let before = expires_at - Duration::MICROSECOND;
    let filter = |now| MessageInboxFilter {
        tenant_id: TENANT,
        user_id: RECIPIENT,
        cursor: None,
        limit: 10,
        unread_only: false,
        unacknowledged_only: false,
        now,
    };
    let visible = messages
        .inbox(filter(before))
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(visible.records.len(), 1);
    assert_eq!(visible.records[0].message.id, message_id);
    assert_eq!(
        visible.records[0].acked_at,
        Some(STORED_PUBLISHED_AT + Duration::SECOND)
    );
    assert_eq!(
        visible.records[0].read_at,
        Some(STORED_PUBLISHED_AT + Duration::seconds(3))
    );
    assert!(
        messages
            .inbox(filter(expires_at))
            .await
            .map_err(|error| error.to_string())?
            .records
            .is_empty()
    );

    let transaction = messages.begin().await.map_err(|error| error.to_string())?;
    assert_eq!(
        transaction
            .delete_expired_batch(before, 10)
            .await
            .map_err(|error| error.to_string())?,
        0
    );
    assert_eq!(
        transaction
            .delete_expired_batch(expires_at, 10)
            .await
            .map_err(|error| error.to_string())?,
        1
    );
    transaction
        .commit(TransactionAuditMode::Skip)
        .await
        .map_err(|error| error.to_string())?;
    for table in [
        "sys_message",
        "sys_message_recipient",
        "sys_message_audience",
    ] {
        require_count(
            database,
            &format!("SELECT COUNT(*) AS value FROM {table}"),
            0,
        )
        .await?;
    }
    Ok(())
}
