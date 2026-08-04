//! 消息仓储的集成测试。

mod common;

use chrono::{Duration, Utc};
use ryframe_db::{
    BackgroundJobRepository, MessageAudienceKind, MessageAudienceSelector, MessageInboxQuery,
    MessageRepository, PublishMessageCommand,
    entities::{message, message_recipient, user},
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, TransactionTrait,
};

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

async fn insert_user(db: &common::TestDatabase, id: i64, status: &str) {
    let username = format!("message-user-{id}");
    user::ActiveModel {
        id: Set(id),
        tenant_id: Set("system".into()),
        username: Set(username.clone()),
        password_hash: Set("test-hash".into()),
        nickname: Set(username.clone()),
        email: Set(format!("{username}@test.local")),
        phone: Set(String::new()),
        avatar: Set(None),
        avatar_file_id: Set(None),
        preferred_locale: Set(None),
        status: Set(status.into()),
        authorization_version: Set(1),
        dept_id: Set(None),
        remark: Set(None),
        login_ip: Set(None),
        login_date: Set(None),
        del_flag: Set(user::Model::DEL_FLAG_NORMAL.into()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
    }
    .insert(db)
    .await
    .expect("insert message recipient user");
}

fn publish_command(now: chrono::DateTime<Utc>) -> PublishMessageCommand {
    PublishMessageCommand {
        tenant_id: "system".into(),
        topic: "system.test".into(),
        title_text: Some("测试消息".into()),
        body_text: Some("测试正文".into()),
        title_key: None,
        body_key: None,
        args_json: None,
        severity: message::Model::SEVERITY_INFO.into(),
        payload_json: None,
        source_type: None,
        source_id: None,
        created_by: 1,
        published_at: now,
        expires_at: now + Duration::days(1),
        audiences: vec![MessageAudienceSelector {
            kind: MessageAudienceKind::Tenant,
            target_id: 0,
        }],
    }
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

#[tokio::test]
async fn publish_uses_insert_select_and_rejects_recipient_overflow() {
    let db = common::setup_test_db().await;
    for id in 11..=13 {
        insert_user(&db, id, user::Model::STATUS_NORMAL).await;
    }
    insert_user(&db, 14, user::Model::STATUS_DISABLED).await;

    let transaction = db.begin().await.expect("begin recipient limit transaction");
    let error = MessageRepository
        .publish_in_transaction(&transaction, publish_command(Utc::now()), 2)
        .await
        .expect_err("three enabled recipients must exceed limit two");
    assert!(error.to_string().contains("消息收件人数不能超过 2"));
    assert_eq!(
        message_recipient::Entity::find()
            .count(&transaction)
            .await
            .expect("count bounded recipient snapshot"),
        3,
        "超限检测只应写入 max + 1 条后立即拒绝"
    );
    transaction
        .rollback()
        .await
        .expect("rollback rejected publication");
    assert_eq!(
        message_recipient::Entity::find()
            .count(&db)
            .await
            .expect("count rolled back recipients"),
        0
    );

    let transaction = db.begin().await.expect("begin successful publication");
    let published = MessageRepository
        .publish_in_transaction(&transaction, publish_command(Utc::now()), 3)
        .await
        .expect("three enabled recipients fit the configured limit");
    assert_eq!(published.recipient_count, 3);
    let user_ids = message_recipient::Entity::find()
        .filter(message_recipient::Column::MessageId.eq(published.message.id))
        .order_by_asc(message_recipient::Column::UserId)
        .all(&transaction)
        .await
        .expect("read recipient snapshot")
        .into_iter()
        .map(|recipient| recipient.user_id)
        .collect::<Vec<_>>();
    assert_eq!(user_ids, vec![11, 12, 13]);
    transaction.commit().await.expect("commit publication");
}

#[tokio::test]
async fn committed_publication_is_immediately_visible_from_inbox() {
    let db = common::setup_test_db().await;
    insert_user(&db, 1, user::Model::STATUS_NORMAL).await;
    let published_at = BackgroundJobRepository
        .database_utc_now(&db)
        .await
        .expect("read database clock before publication");
    let mut command = publish_command(published_at);
    command.audiences = vec![MessageAudienceSelector {
        kind: MessageAudienceKind::User,
        target_id: 1,
    }];

    let transaction = db.begin().await.expect("begin publication transaction");
    let published = MessageRepository
        .publish_in_transaction(&transaction, command, 1)
        .await
        .expect("publish message for one user");
    transaction.commit().await.expect("commit publication");

    let query_time = BackgroundJobRepository
        .database_utc_now(&db)
        .await
        .expect("read database clock after publication");
    let inbox = MessageRepository
        .inbox(
            &db,
            MessageInboxQuery {
                tenant_id: "system",
                user_id: 1,
                cursor: None,
                limit: 100,
                unread_only: false,
                unacknowledged_only: false,
                now: query_time,
            },
        )
        .await
        .expect("read committed publication from inbox");

    assert!(
        inbox
            .records
            .iter()
            .any(|record| record.message.id == published.message.id),
        "committed publication must be visible to a fresh inbox query"
    );
}
