//! 显式目标完整库存的真实 MySQL 回归；复用协议测试的 opt-in 与精确 schema 清理。
use std::{collections::BTreeSet, sync::Arc, time::Duration};

use ryframe_application::ports::backup::{
    BackupDatabaseKind, BackupDatabaseVerifier, BackupTableDigest, DatabaseTargetInventory,
};
use ryframe_config::{
    SqlLogLevel, TenantDataConfig, TenantDatabaseTargetConfig, TenantDatabaseTargetKind,
    TenantDatabaseTargetMode,
};
use ryframe_db::{ControlDatabaseCluster, connection, resource_ownership};
use ryframe_tenant_db::{TenantDatabaseRouter, application_ports::backup};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

#[path = "../../ryframe-db/tests/support/mysql.rs"]
mod mysql;
use mysql::{database_config, db_error, execute, require_count, run_mysql_test};

const SCOPE: &str = "target-inventory-integration";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn complete_inventory_is_read_only_and_rejects_drift_on_real_mysql() {
    run_mysql_test("inventoryctl", |control| async move {
        run_mysql_test("inventoryshared", |shared| async move {
            run_mysql_test("inventoryded", |dedicated| async move {
                exercise_inventory(control, shared, dedicated).await
            })
            .await;
            Ok(())
        })
        .await;
        Ok(())
    })
    .await;
}

async fn exercise_inventory(
    control: DatabaseConnection,
    shared: DatabaseConnection,
    dedicated: DatabaseConnection,
) -> Result<(), String> {
    ryframe_db::migration::up(&control)
        .await
        .map_err(db_error)?;
    for database in [&control, &shared, &dedicated] {
        ryframe_tenant_db::migration::up(database)
            .await
            .map_err(db_error)?;
        resource_ownership::ensure_resource_ownership(database, SCOPE, "tenant-data")
            .await
            .map_err(db_error)?;
    }
    resource_ownership::ensure_resource_ownership(&control, SCOPE, "control")
        .await
        .map_err(db_error)?;
    let mut config = database_config(&database_name(&control).await?);
    // combined 采集必须在持有只读事务时复用该连接，不能再从同一个池借连接。
    config.max_connections = 1;
    let single_connection = connection::connect(&config)
        .await
        .map_err(|error| error.to_string())?;
    let cluster = ControlDatabaseCluster::single(single_connection.clone());
    let targets = TenantDataConfig {
        targets: vec![
            mysql_target(
                "inventory-shared",
                &shared,
                TenantDatabaseTargetMode::Shared,
            )
            .await?,
            mysql_target(
                "inventory-dedicated",
                &dedicated,
                TenantDatabaseTargetMode::Dedicated,
            )
            .await?,
        ],
        ..Default::default()
    };
    let router = Arc::new(
        TenantDatabaseRouter::new(cluster.clone(), &targets, SqlLogLevel::Off, 200)
            .map_err(|error| error.to_string())?,
    );
    let verifier = backup::verifier(cluster, router, SCOPE.into());
    let result = check_targets(verifier.as_ref(), &control, &shared, &dedicated).await;
    drop(verifier);
    single_connection.close().await.map_err(db_error)?;
    result
}

async fn mysql_target(
    key: &str,
    database: &DatabaseConnection,
    mode: TenantDatabaseTargetMode,
) -> Result<TenantDatabaseTargetConfig, String> {
    let config = database_config(&database_name(database).await?);
    Ok(TenantDatabaseTargetConfig {
        key: key.into(),
        mode,
        kind: TenantDatabaseTargetKind::Mysql,
        max_connections: Some(1),
        host: Some(config.host),
        port: Some(config.port),
        database: Some(config.database),
        username: Some(config.username),
        password_env: Some("RYFRAME_MYSQL_PASSWORD".into()),
        tls_mode: Some(config.tls_mode),
        ..TenantDatabaseTargetConfig::shared_control()
    })
}

async fn check_targets(
    verifier: &dyn BackupDatabaseVerifier,
    control: &DatabaseConnection,
    shared: &DatabaseConnection,
    dedicated: &DatabaseConnection,
) -> Result<(), String> {
    assert!(inventory(verifier, "not-registered").await?.is_err());
    for (key, database, combined, is_shared) in [
        ("shared-control", control, true, true),
        ("inventory-shared", shared, false, true),
        ("inventory-dedicated", dedicated, false, false),
    ] {
        let first = inventory(verifier, key)
            .await?
            .map_err(|error| error.to_string())?;
        let second = inventory(verifier, key)
            .await?
            .map_err(|error| error.to_string())?;
        assert_eq!(first, second, "{key} 重复只读采集改变了完整前像");
        assert_complete_inventory(&first, database, combined, is_shared).await?;
        check_ownership_rejection(verifier, key, database).await?;
        check_schema_rejection(verifier, key, database).await?;
        assert_eq!(
            first,
            inventory(verifier, key)
                .await?
                .map_err(|error| error.to_string())?,
            "{key} 拒绝异常后未恢复原始完整前像"
        );
    }
    check_content_digest(verifier, control).await
}

async fn inventory(
    verifier: &dyn BackupDatabaseVerifier,
    key: &str,
) -> Result<ryframe_kernel::AppResult<DatabaseTargetInventory>, String> {
    tokio::time::timeout(Duration::from_secs(20), verifier.target_inventory(key))
        .await
        .map_err(|_| format!("{key} 库存超时；检查单连接池内是否重复借用连接"))
}

async fn assert_complete_inventory(
    inventory: &DatabaseTargetInventory,
    database: &DatabaseConnection,
    combined: bool,
    shared: bool,
) -> Result<(), String> {
    assert_eq!(inventory.database.database, database_name(database).await?);
    assert!(!inventory.database.server_uuid.is_empty());
    assert_eq!(inventory.database.shared, shared);
    assert_eq!(
        inventory.database.kind,
        if combined {
            BackupDatabaseKind::Combined
        } else {
            BackupDatabaseKind::Tenant
        }
    );
    let mut expected_preserved = vec!["ryframe_resource_ownership", "seaql_tenant_data_migrations"];
    if combined {
        expected_preserved.extend([
            "seaql_migrations",
            "sys_backup_set",
            "sys_backup_resource",
            "sys_restore_run",
        ]);
        assert_eq!(inventory.database.placements.len(), 1);
        assert_eq!(inventory.database.placements[0].tenant_id, "system");
    } else {
        assert!(inventory.database.placements.is_empty());
    }
    assert_eq!(
        inventory
            .preserved_tables
            .iter()
            .map(|table| table.table.as_str())
            .collect::<BTreeSet<_>>(),
        expected_preserved.into_iter().collect()
    );
    let tables = inventory
        .database
        .tables
        .iter()
        .chain(&inventory.preserved_tables)
        .collect::<Vec<_>>();
    let names = tables
        .iter()
        .map(|table| table.table.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(tables.len(), names.len(), "完整目录不能重复计算表");
    assert_eq!(
        names,
        table_names(database)
            .await?
            .iter()
            .map(String::as_str)
            .collect()
    );
    for table in &tables {
        assert_eq!(table.sha256.len(), 64, "{} 缺少完整摘要", table.table);
        assert!(table.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        require_count(
            database,
            &format!("SELECT COUNT(*) AS value FROM `{}`", table.table),
            i64::try_from(table.rows).map_err(|error| error.to_string())?,
        )
        .await?;
    }
    assert_eq!(
        digest(inventory, "ryframe_resource_ownership").rows,
        if combined { 2 } else { 1 }
    );
    assert!(digest(inventory, "seaql_tenant_data_migrations").rows > 0);
    assert_eq!(digest(inventory, "biz_tenant_target_slot").rows, 1);
    if combined {
        assert!(digest(inventory, "seaql_migrations").rows > 0);
        for table in ["sys_backup_set", "sys_backup_resource", "sys_restore_run"] {
            assert_eq!(digest(inventory, table).rows, 0);
        }
    }
    Ok(())
}

async fn check_ownership_rejection(
    verifier: &dyn BackupDatabaseVerifier,
    key: &str,
    database: &DatabaseConnection,
) -> Result<(), String> {
    for kind in if key == "shared-control" {
        &["control", "tenant-data"][..]
    } else {
        &["tenant-data"][..]
    } {
        for (column, expected) in [
            ("marker", resource_ownership::marker(SCOPE, kind)),
            ("scope_id", SCOPE.to_owned()),
        ] {
            execute(database, &format!(
                "UPDATE ryframe_resource_ownership SET {column}='invalid-owner', updated_at=updated_at WHERE resource_kind='{kind}'"
            )).await?;
            let rejected = inventory(verifier, key).await?;
            execute(database, &format!(
                "UPDATE ryframe_resource_ownership SET {column}='{expected}', updated_at=updated_at WHERE resource_kind='{kind}'"
            )).await?;
            assert!(rejected.is_err(), "{key} 接受了错误 {kind} {column}");
        }
    }
    resource_ownership::ensure_resource_ownership(database, SCOPE, "unexpected")
        .await
        .map_err(db_error)?;
    let rejected = inventory(verifier, key).await?;
    execute(
        database,
        "DELETE FROM ryframe_resource_ownership WHERE resource_kind='unexpected'",
    )
    .await?;
    assert!(rejected.is_err(), "{key} 接受了额外 owner 行");
    Ok(())
}

async fn check_schema_rejection(
    verifier: &dyn BackupDatabaseVerifier,
    key: &str,
    database: &DatabaseConnection,
) -> Result<(), String> {
    for (change, restore) in [
        (
            "CREATE TABLE inventory_unexpected (id BIGINT PRIMARY KEY)",
            "DROP TABLE inventory_unexpected",
        ),
        (
            "CREATE VIEW inventory_unexpected_view AS SELECT slot_id FROM biz_tenant_target_slot",
            "DROP VIEW inventory_unexpected_view",
        ),
        (
            "ALTER TABLE biz_tenant_target_slot ADD COLUMN unexpected_value INT NULL",
            "ALTER TABLE biz_tenant_target_slot DROP COLUMN unexpected_value",
        ),
    ] {
        execute(database, change).await?;
        let rejected = inventory(verifier, key).await?;
        execute(database, restore).await?;
        assert!(rejected.is_err(), "{key} 未拒绝 schema 漂移: {change}");
    }
    Ok(())
}

async fn check_content_digest(
    verifier: &dyn BackupDatabaseVerifier,
    control: &DatabaseConnection,
) -> Result<(), String> {
    let before = inventory(verifier, "shared-control")
        .await?
        .map_err(|error| error.to_string())?;
    execute(control, "UPDATE sys_post SET sort=sort+1 WHERE id=1").await?;
    let changed = inventory(verifier, "shared-control")
        .await?
        .map_err(|error| error.to_string())?;
    execute(control, "UPDATE sys_post SET sort=sort-1 WHERE id=1").await?;
    assert_eq!(
        digest(&before, "sys_post").rows,
        digest(&changed, "sys_post").rows
    );
    assert_ne!(
        digest(&before, "sys_post").sha256,
        digest(&changed, "sys_post").sha256
    );
    Ok(())
}

fn digest<'a>(inventory: &'a DatabaseTargetInventory, name: &str) -> &'a BackupTableDigest {
    inventory
        .database
        .tables
        .iter()
        .chain(&inventory.preserved_tables)
        .find(|table| table.table == name)
        .unwrap_or_else(|| panic!("完整库存缺少 {name}"))
}

async fn database_name(database: &DatabaseConnection) -> Result<String, String> {
    database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT DATABASE() AS value",
        ))
        .await
        .map_err(db_error)?
        .ok_or("缺少当前数据库")?
        .try_get("", "value")
        .map_err(db_error)
}

async fn table_names(database: &DatabaseConnection) -> Result<BTreeSet<String>, String> {
    database.query_all_raw(Statement::from_string(DbBackend::MySql,
        "SELECT TABLE_NAME AS value FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE() ORDER BY TABLE_NAME"))
        .await.map_err(db_error)?.into_iter()
        .map(|row| row.try_get("", "value").map_err(db_error)).collect()
}
