//! 消息仓储的集成测试。

mod common;

use chrono::{Duration, Utc};
use ryframe_db::{
    MessageInboxQuery, MessageRepository,
    entities::{message, message_recipient},
};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

async fn insert_message_with_recipient(
    db: &common::TestDatabase,
    id: i64,
    expires_at: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
) {
    message::ActiveModel {
        id: Set(id),
        tenant_id: Set("system".into()),
        topic: Set("system.test".into()),
        title_text: Set(Some(format!("消息 {id}"))),
        body_text: Set(Some("测试正文".into())),
        title_key: Set(None),
        body_key: Set(None),
        args_json: Set(None),
        severity: Set(message::Model::SEVERITY_INFO.into()),
        payload_json: Set(None),
        source_type: Set(None),
        source_id: Set(None),
        created_by: Set(Some(1)),
        published_at: Set(now - Duration::minutes(1)),
        expires_at: Set(Some(expires_at)),
        created_at: Set(now - Duration::minutes(1)),
        updated_at: Set(now - Duration::minutes(1)),
    }
    .insert(db)
    .await
    .expect("insert message");
    message_recipient::ActiveModel {
        message_id: Set(id),
        user_id: Set(1),
        tenant_id: Set("system".into()),
        created_at: Set(now - Duration::minutes(1)),
        enqueued_at: Set(None),
        acked_at: Set(None),
        read_at: Set(None),
    }
    .insert(db)
    .await
    .expect("insert recipient");
}

async fn recipient(db: &common::TestDatabase, message_id: i64) -> message_recipient::Model {
    message_recipient::Entity::find_by_id((message_id, 1))
        .one(db)
        .await
        .expect("query recipient")
        .expect("recipient exists")
}

#[tokio::test]
async fn inbox_skips_many_expired_messages_without_losing_valid_page() {
    let db = common::setup_test_db().await;
    let now = Utc::now();
    for id in (91..=100).rev() {
        insert_message_with_recipient(&db, id, now - Duration::seconds(1), now).await;
    }
    for id in [90, 89, 88] {
        insert_message_with_recipient(&db, id, now + Duration::days(1), now).await;
    }

    let page = MessageRepository
        .inbox(
            &db,
            MessageInboxQuery {
                tenant_id: "system",
                user_id: 1,
                cursor: None,
                limit: 2,
                unread_only: false,
                unacknowledged_only: false,
                now,
            },
        )
        .await
        .expect("query inbox");

    assert_eq!(
        page.records
            .iter()
            .map(|record| record.message.id)
            .collect::<Vec<_>>(),
        vec![90, 89]
    );
    assert_eq!(page.next_cursor, Some(89));
    assert_eq!(
        MessageRepository
            .unread_count(&db, "system", 1, now)
            .await
            .expect("count unread messages"),
        3
    );
}

#[tokio::test]
async fn recipient_state_updates_ignore_expired_messages() {
    let db = common::setup_test_db().await;
    let now = Utc::now();
    for id in [1, 3, 5] {
        insert_message_with_recipient(&db, id, now - Duration::seconds(1), now).await;
    }
    for id in [2, 4, 6] {
        insert_message_with_recipient(&db, id, now + Duration::days(1), now).await;
    }

    assert_eq!(
        MessageRepository
            .acknowledge(&db, "system", 1, &[1, 2], now)
            .await
            .expect("acknowledge messages"),
        1
    );
    assert!(
        !MessageRepository
            .mark_read(&db, "system", 1, 3, now)
            .await
            .expect("mark expired message read")
    );
    assert!(
        MessageRepository
            .mark_read(&db, "system", 1, 4, now)
            .await
            .expect("mark active message read")
    );
    assert_eq!(
        MessageRepository
            .mark_all_read(&db, "system", 1, now)
            .await
            .expect("mark all active messages read"),
        2
    );

    for id in [1, 3, 5] {
        let expired = recipient(&db, id).await;
        assert!(
            expired.acked_at.is_none(),
            "expired message {id} was acknowledged"
        );
        assert!(
            expired.read_at.is_none(),
            "expired message {id} was marked read"
        );
    }
    for id in [2, 4, 6] {
        let active = recipient(&db, id).await;
        assert!(
            active.acked_at.is_some(),
            "active message {id} was not acknowledged"
        );
        assert!(
            active.read_at.is_some(),
            "active message {id} was not marked read"
        );
    }
}
