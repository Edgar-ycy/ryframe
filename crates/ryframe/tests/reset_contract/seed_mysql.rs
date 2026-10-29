//! 仅在新建且受管的 schema 内变更；每个 SQL 负例回滚后保留规范投影种子。
use sea_orm::{DatabaseConnection, TransactionTrait};

use super::super::operations::verify_empty_control_resources;

#[path = "../../../ryframe-db/tests/support/mysql.rs"]
mod support;

const MUTATIONS: &[(&str, &str)] = &[
    ("event-type", "event_type = 'other.event'"),
    ("event-type-case", "event_type = UPPER(event_type)"),
    ("aggregate-type", "aggregate_type = 'other_aggregate'"),
    ("aggregate-id", "aggregate_id = 'other-tenant'"),
    ("dedupe", "dedupe_key = CONCAT('other:', dedupe_key)"),
    ("unknown-tenant", "tenant_id = 'missing-tenant'"),
    ("null-tenant", "tenant_id = NULL"),
    (
        "payload-tenant",
        "payload = JSON_SET(payload, '$.tenant_id', 'other-tenant')",
    ),
    (
        "payload-version",
        "payload = JSON_SET(payload, '$.source_version', 99)",
    ),
    (
        "payload-version-type",
        "payload = JSON_SET(payload, '$.source_version', '1')",
    ),
    ("payload-extra", "payload = JSON_SET(payload, '$.extra', 1)"),
    ("status", "status = 'published'"),
    ("attempts", "attempts = 1"),
    ("max-attempts", "max_attempts = 5"),
    ("lease-owner", "lease_owner = 'test-worker'"),
    ("lease-until", "lease_until = created_at"),
    ("published", "published_at = created_at"),
    ("error", "last_error = 'test-error'"),
    (
        "traceparent",
        "traceparent = '00-11111111111111111111111111111111-1111111111111111-01'",
    ),
    ("tracestate", "tracestate = 'test=1'"),
    (
        "available-time",
        "available_at = DATE_ADD(created_at, INTERVAL 1 SECOND)",
    ),
    (
        "updated-time",
        "updated_at = DATE_ADD(created_at, INTERVAL 1 SECOND)",
    ),
];

#[tokio::test]
async fn reset_projection_seed_sql_rejects_mutations_and_preserves_pristine_baseline() {
    crate::crypto::install_crypto_provider().unwrap();
    support::run_mysql_test("reset-projection", |database| async move {
        let seeds = super::seed_credentials(Some(super::ADMIN_PASSWORD))
            .map_err(|error| error.to_string())?;
        let hashes = ryframe_db::migration::BootstrapHashes::new(seeds.admin_hash, seeds.user_hash)
            .map_err(support::db_error)?;
        ryframe_db::migration::up(&database, Some(&hashes))
            .await
            .map_err(support::db_error)?;
        verify_empty_control_resources(&database)
            .await
            .map_err(|error| error.to_string())?;
        for (label, assignment) in MUTATIONS {
            let sql = format!("UPDATE sys_outbox_event SET {assignment} ORDER BY id LIMIT 1");
            rejects(&database, label, &sql, "初始化投影种子").await?;
        }
        rejects(
            &database,
            "missing",
            "DELETE FROM sys_outbox_event ORDER BY id LIMIT 1",
            "初始化投影种子",
        )
        .await?;
        rejects(
            &database,
            "extra",
            "INSERT INTO sys_outbox_event
            (id, tenant_id, event_type, aggregate_type, aggregate_id, payload, status,
             available_at, attempts, max_attempts, dedupe_key, created_at, updated_at)
            SELECT UUID_SHORT(), tenant_id, CONCAT(event_type, '.extra'), aggregate_type,
                aggregate_id, payload, status, available_at, attempts, max_attempts,
                CONCAT('extra:', dedupe_key), created_at, updated_at
            FROM sys_outbox_event ORDER BY id LIMIT 1",
            "初始化投影种子",
        )
        .await?;
        rejects(
            &database,
            "runtime-table",
            "INSERT INTO sys_login_info
            (id, tenant_id, user_name, ipaddr, login_time)
            SELECT 1, tenant_id, 'reset-verification', '127.0.0.1', UTC_TIMESTAMP()
            FROM sys_tenant ORDER BY tenant_id LIMIT 1",
            "sys_login_info",
        )
        .await?;
        verify_empty_control_resources(&database)
            .await
            .map_err(|error| error.to_string())
    })
    .await;
}

async fn rejects(
    database: &DatabaseConnection,
    label: &str,
    sql: &str,
    expected: &str,
) -> Result<(), String> {
    let transaction = database.begin().await.map_err(support::db_error)?;
    let result = async {
        support::execute(&transaction, sql)
            .await
            .map_err(|error| format!("{label} 变更 SQL 失败，不能计为验证器拒绝：{error}"))?;
        support::require_count(&transaction, "SELECT ROW_COUNT() AS value", 1).await?;
        let error = match verify_empty_control_resources(&transaction).await {
            Ok(()) => return Err(format!("{label} 变更已写入，但验证器没有拒绝")),
            Err(error) => error.to_string(),
        };
        if !error.contains(expected) {
            return Err(format!("{label} 未命中预期种子或运行表约束：{error}"));
        }
        Ok(())
    }
    .await;
    transaction.rollback().await.map_err(support::db_error)?;
    result
}
