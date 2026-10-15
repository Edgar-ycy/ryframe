use super::{adoption::*, *};
use ryframe_sdk::{
    BusinessAdoptionState, apply_business_adoption, inspect_business_adoption,
    preview_business_adoption,
};

fn competing() -> SeaOrmBusinessMigration {
    SeaOrmBusinessMigration::new(
        "competing-module",
        "m20261014_000000_example",
        CHECKSUM,
        BusinessMigrationSchema {
            before_fingerprint: None,
            after_fingerprint: CHECKSUM,
            tables: &[BusinessTableSchema {
                table: "biz_sql_example",
                before: None,
                after: TABLE_SCHEMA,
            }],
        },
        BusinessMigrationScope::Control,
        Initial { mismatched: false },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_controllers_cannot_both_adopt_the_same_table() {
    mysql::run_mysql_test("adoptownership", |database| async move {
        mysql::execute(&database, DDL).await?;
        empty_ledger(&database).await?;
        let initial = migration(false);
        let other = competing();
        let first = preview_business_adoption(
            &database,
            BusinessMigrationScope::Control,
            initial.module(),
            &[&initial],
        )
        .await
        .map_err(message)?;
        let second = preview_business_adoption(
            &database,
            BusinessMigrationScope::Control,
            other.module(),
            &[&other],
        )
        .await
        .map_err(message)?;
        let left_migrations: [&dyn BusinessMigration; 1] = [&initial];
        let right_migrations: [&dyn BusinessMigration; 1] = [&other];
        let (left, right) = tokio::join!(
            apply_business_adoption(&database, &first, &left_migrations),
            apply_business_adoption(&database, &second, &right_migrations),
        );
        assert_eq!(
            usize::from(left.is_ok()) + usize::from(right.is_ok()),
            1,
            "left={left:?}, right={right:?}"
        );
        mysql::require_count(
            &database,
            "SELECT COUNT(*) AS value FROM ryframe_business_migration",
            1,
        )
        .await?;
        let (plan, winner, loser) = if left.is_ok() {
            (&first, &initial, &other)
        } else {
            (&second, &other, &initial)
        };
        assert_eq!(
            inspect_business_adoption(&database, plan, &[winner])
                .await
                .map_err(message)?,
            BusinessAdoptionState::Applied
        );
        let failure = preview_business_adoption(
            &database,
            BusinessMigrationScope::Control,
            loser.module(),
            &[loser],
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            failure.contains("未注册") || failure.contains("归属"),
            "{failure}"
        );
        assert_eq!(
            ryframe_db::schema::canonical_table_schema(&database, "biz_sql_example")
                .await
                .map_err(mysql::db_error)?,
            TABLE_SCHEMA
        );
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_adoption_releases_transaction_and_does_not_leave_partial_records() {
    mysql::run_mysql_test("adoptcancel", |database| async move {
        mysql::execute(&database, DDL).await?;
        empty_ledger(&database).await?;
        let initial = migration(false);
        let plan = preview_business_adoption(&database, BusinessMigrationScope::Control, initial.module(), &[&initial]).await.map_err(message)?;
        mysql::execute(&database, "CREATE TABLE fixture_ledger_gate (id BIGINT PRIMARY KEY)").await?;
        mysql::execute(&database, "INSERT INTO fixture_ledger_gate VALUES(1)").await?;
        database.execute_unprepared("CREATE TRIGGER hold_adoption BEFORE INSERT ON ryframe_business_migration FOR EACH ROW BEGIN DECLARE gate_id BIGINT; SELECT id INTO gate_id FROM fixture_ledger_gate WHERE id=1 FOR UPDATE; END").await.map_err(mysql::db_error)?;
        let gate = database.begin().await.map_err(mysql::db_error)?;
        gate.query_one_raw(Statement::from_string(DbBackend::MySql, "SELECT id FROM fixture_ledger_gate WHERE id=1 FOR UPDATE")).await.map_err(mysql::db_error)?;
        let connection = database.clone();
        let saved = plan.clone();
        let applying = tokio::spawn(async move { apply_business_adoption(&connection, &saved, &[&migration(false)]).await });
        let blocked = wait_for_database_state(&database, "SELECT COUNT(*) AS value FROM performance_schema.data_lock_waits w JOIN performance_schema.data_locks l ON l.ENGINE_LOCK_ID=w.REQUESTING_ENGINE_LOCK_ID AND l.ENGINE=w.ENGINE WHERE l.OBJECT_SCHEMA=DATABASE() AND l.OBJECT_NAME='fixture_ledger_gate'", "接管 INSERT 被 gate 阻塞").await;
        applying.abort();
        let cancelled = applying.await;
        gate.rollback().await.map_err(mysql::db_error)?;
        blocked?;
        assert!(cancelled.unwrap_err().is_cancelled());
        database.execute_unprepared("DROP TRIGGER hold_adoption").await.map_err(mysql::db_error)?;
        assert_eq!(inspect_business_adoption(&database, &plan, &[&initial]).await.map_err(message)?, BusinessAdoptionState::Unapplied);
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration", 0).await?;
        assert_eq!(apply_business_adoption(&database, &plan, &[&initial]).await.map_err(message)?.registered, 1);
        Ok(())
    }).await;
}
