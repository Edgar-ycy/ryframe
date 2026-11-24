pub use ryframe_application::{PersistenceTransaction, TransactionAuditMode};
pub mod unsigned_auto;
pub mod unsigned_snowflake;

use unsigned_auto::{fake::OrderFakePersistence as AutoFake, port::OrderPersistencePort as _};
use unsigned_snowflake::{
    fake::OrderFakePersistence as SnowflakeFake, port::OrderPersistencePort as _,
};

fn record(id: u64) -> unsigned_auto::model::OrderRecord {
    unsigned_auto::model::OrderRecord {
        tenant_id: "tenant-a".into(),
        id,
        name: format!("order-{id}"),
        status: 1,
        created_at: time::OffsetDateTime::now_utc(),
        updated_at: None,
        del_flag: 0,
    }
}

#[tokio::test]
async fn unsigned_auto_ids_cross_signed_limit_and_fail_closed_at_capacity() {
    let fake = AutoFake::default();
    fake.insert_record("tenant-a", record(i64::MAX as u64));
    let transaction = fake.begin("tenant-a").await.unwrap();
    let inserted = transaction.insert(record(0)).await.unwrap();
    assert_eq!(inserted.id, i64::MAX as u64 + 1);
    transaction
        .commit(TransactionAuditMode::Skip)
        .await
        .unwrap();
    assert!(
        fake.find_by_id("tenant-a", inserted.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        fake.find_by_id("tenant-b", inserted.id)
            .await
            .unwrap()
            .is_none()
    );
    fake.insert_record("tenant-a", record(u64::MAX));
    let transaction = fake.begin("tenant-a").await.unwrap();
    assert!(transaction.insert(record(0)).await.is_err());
    transaction.rollback().await.unwrap();
    assert!(
        fake.find_by_id("tenant-a", u64::MAX)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn unsigned_explicit_ids_preserve_maximum_and_transaction_visibility() {
    let fake = SnowflakeFake::default();
    let row = unsigned_snowflake::model::OrderInsert {
        tenant_id: "tenant-a".into(),
        id: u64::MAX,
        name: "maximum".into(),
        status: 1,
        del_flag: 0,
    };
    let rollback = fake.begin("tenant-a").await.unwrap();
    rollback.insert(row.clone()).await.unwrap();
    rollback.rollback().await.unwrap();
    assert!(
        fake.find_by_id("tenant-a", u64::MAX)
            .await
            .unwrap()
            .is_none()
    );
    let transaction = fake.begin("tenant-a").await.unwrap();
    let inserted = transaction.insert(row).await.unwrap();
    let created_at = inserted.created_at;
    assert_eq!(created_at.microsecond() % 1000, 0);
    assert!(inserted.updated_at.is_some());
    let mut changed = inserted;
    let ignored = time::OffsetDateTime::UNIX_EPOCH;
    changed.created_at = ignored;
    changed.updated_at = Some(ignored);
    changed.name = "changed".into();
    let updated = transaction.update(changed).await.unwrap();
    assert_eq!(updated.created_at, created_at);
    assert!(updated.updated_at.unwrap() >= created_at);
    transaction
        .commit(TransactionAuditMode::Skip)
        .await
        .unwrap();
    assert_eq!(
        fake.find_by_id("tenant-a", u64::MAX)
            .await
            .unwrap()
            .unwrap()
            .id,
        u64::MAX
    );
    assert!(
        fake.find_by_id("tenant-b", u64::MAX)
            .await
            .unwrap()
            .is_none()
    );
}
