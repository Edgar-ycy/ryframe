use super::*;
use ryframe_config::{SqlLogLevel, TenantDataConfig, TenantDatabaseTargetConfig};
use ryframe_tenant_db::TenantDatabaseTargetRegistry;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dedicated_aliases_of_one_physical_database_fail_before_any_migration() {
    mysql::run_mysql_test("migrationalias", |database| async move {
        let identity = ryframe_db::connection::physical_identity(&database)
            .await
            .map_err(mysql::db_error)?;
        let configured =
            mysql::MySqlTestConfiguration::load().unwrap_or_else(|error| panic!("{error}"));
        let primary = configured
            .connection(&identity.database, None)
            .unwrap_or_else(|error| panic!("{error}"));
        let mut first = target("first", &primary);
        first.mode = ryframe_config::TenantDatabaseTargetMode::Dedicated;
        let mut second = target("second", &primary);
        second.mode = ryframe_config::TenantDatabaseTargetMode::Dedicated;
        if primary.host == "127.0.0.1" {
            second.host = Some("localhost".into());
        }
        let config = TenantDataConfig {
            targets: vec![first, second],
            ..TenantDataConfig::default()
        };
        let registry =
            TenantDatabaseTargetRegistry::new(&config, SqlLogLevel::Off, 200).map_err(message)?;
        let context = diagnostic::Context {
            directory: std::path::Path::new(".local-tests/mysql/config"),
            environment: Some(ryframe_config::Environment::Test),
            config: None,
            command: "tenant-data verify",
            target: None,
        };
        let result = migration_targets::preflight(
            &registry,
            &primary,
            SqlLogLevel::Off,
            200,
            &["first".into(), "second".into()],
            &context,
        )
        .await;
        let plan = migration_targets::preflight(
            &registry,
            &primary,
            SqlLogLevel::Off,
            200,
            &["shared-control".into()],
            &context,
        )
        .await;
        for key in ["first", "second"] {
            registry
                .acquire(key)
                .await
                .map_err(message)?
                .connection()
                .clone()
                .close()
                .await
                .map_err(mysql::db_error)?;
        }
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("first")
                && error.contains("second")
                && error.contains("同一 MySQL")
                && error.contains("尚未执行"),
            "{error}"
        );
        let plan = plan.map_err(message)?;
        assert_eq!(plan.get("shared-control"), Some(&identity));
        migration_targets::verify_identity(&database, "shared-control", &identity)
            .await
            .map_err(message)?;
        let mut changed = identity;
        changed.server_uuid.push_str("-changed");
        assert!(
            migration_targets::verify_identity(&database, "shared-control", &changed)
                .await
                .unwrap_err()
                .to_string()
                .contains("预检后发生变化")
        );
        mysql::require_count(
            &database,
            "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE()",
            0,
        )
        .await?;
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn distinct_database_names_on_one_server_remain_independent_targets() {
    mysql::run_mysql_test("migrationdifferent", |database| async move {
        let first = ryframe_db::connection::physical_identity(&database)
            .await
            .map_err(mysql::db_error)?;
        mysql::run_mysql_test("migrationdifferentother", |other| async move {
            let second = ryframe_db::connection::physical_identity(&other)
                .await
                .map_err(mysql::db_error)?;
            assert_eq!(first.server_uuid, second.server_uuid);
            assert_ne!(first.database, second.database);
            migration_targets::reject_duplicates(&std::collections::BTreeMap::from([
                ("first".into(), first),
                ("second".into(), second),
            ]))
            .map_err(message)?;
            Ok(())
        })
        .await;
        Ok(())
    })
    .await;
}

fn target(key: &str, connection: &ryframe_config::DbConnection) -> TenantDatabaseTargetConfig {
    TenantDatabaseTargetConfig {
        key: key.into(),
        display_name: None,
        kind: ryframe_config::TenantDatabaseTargetKind::Mysql,
        mode: ryframe_config::TenantDatabaseTargetMode::Shared,
        host: Some(connection.host.clone()),
        port: Some(connection.port),
        database: Some(connection.database.clone()),
        username: Some(connection.username.clone()),
        password: Some(connection.password.clone()),
        tls_mode: Some(connection.tls_mode),
        tls_ca: connection.tls_ca.clone(),
        tls_client_cert: connection.tls_client_cert.clone(),
        tls_client_key: connection.tls_client_key.clone(),
        max_connections: Some(2),
        ..TenantDatabaseTargetConfig::shared_control()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_aliases_coalesce_only_without_dedicated_slot_identity() {
    mysql::run_mysql_test("sharedaliases", |database| async move {
        let identity = ryframe_db::connection::physical_identity(&database).await.map_err(message)?;
        let primary = mysql::MySqlTestConfiguration::load()?.connection(&identity.database,None)?;
        let config = TenantDataConfig { targets: vec![target("first",&primary),target("second",&primary)],
            ..TenantDataConfig::default() };
        let registry = TenantDatabaseTargetRegistry::new(&config,SqlLogLevel::Off,200).map_err(message)?;
        let context = diagnostic::Context { directory: std::path::Path::new(".local-tests/mysql/config"),
            environment: Some(ryframe_config::Environment::Test), config: None,
            command: "tenant-data up", target: None };
        let keys = ["first".into(),"second".into()];
        let selected = migration_targets::preflight(&registry,&primary,SqlLogLevel::Off,200,&keys,&context)
            .await.map_err(message)?;
        assert_eq!(selected.len(),2);
        assert_eq!(migration_targets::unique_targets(&registry,&selected).await.map_err(message)?.len(),1);
        mysql::execute(&database,ryframe_tenant_db::migration::TENANT_TARGET_SLOT_DDL).await?;
        mysql::execute(&database,"INSERT INTO biz_tenant_target_slot(slot_id,tenant_id,placement_generation,switch_token) VALUES (1,'fixture-tenant',1,'fixture-slot')").await?;
        assert!(migration_targets::preflight(&registry,&primary,SqlLogLevel::Off,200,&keys,&context).await.is_err());
        mysql::execute(&database,"UPDATE biz_tenant_target_slot SET tenant_id=NULL,placement_generation=NULL,switch_token=NULL").await?;
        mysql::execute(&database,ryframe_tenant_db::migration::TENANT_FENCE_DDL).await?;
        mysql::execute(&database,"INSERT INTO biz_tenant_fence(tenant_id,target_key,placement_generation,state,switch_token) VALUES ('fixture-a','first',1,'active','fixture-a'),('fixture-b','second',1,'active','fixture-b')").await?;
        assert!(migration_targets::preflight(&registry,&primary,SqlLogLevel::Off,200,&keys,&context).await.is_err());
        mysql::execute(&database,"DELETE FROM biz_tenant_fence WHERE tenant_id='fixture-a'").await?;
        let selected = migration_targets::preflight(&registry,&primary,SqlLogLevel::Off,200,&keys,&context).await.map_err(message)?;
        assert_eq!(migration_targets::unique_targets(&registry,&selected).await.map_err(message)?,vec!["second".to_owned()]);
        Ok(())
    }).await;
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[test]
fn operation_and_close_failures_keep_both_stages_and_conservative_write_state() {
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".local-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let root = tempfile::Builder::new()
        .prefix("migration-cli-")
        .tempdir_in(parent)
        .unwrap();
    let selected = root.path().join("config");
    configuration_fixture::write_configuration(&selected, 8080, 9091, None);
    let config =
        ryframe_config::AppConfig::load(&selected, ryframe_config::Environment::Test).unwrap();
    let context = diagnostic::Context {
        directory: &selected,
        environment: Some(config.environment),
        config: Some(&config),
        command: "control up",
        target: None,
    };
    let before = context
        .error(
            "连接池建立",
            diagnostic::Writes::None,
            &std::io::Error::other("connect failed"),
        )
        .to_string();
    assert!(before.contains("当前命令尚未发起"));
    assert!(!before.contains("可能已部分提交"));
    let file = context
        .error(
            "文件发布",
            diagnostic::Writes::File,
            &std::io::Error::other("publish failed"),
        )
        .to_string();
    assert!(file.contains("目录、临时文件或目标文件可能已创建"));
    assert!(!file.contains("尚未发起"));
    let operation = context.error(
        "业务迁移执行",
        diagnostic::Writes::Database,
        &std::io::Error::other("operation failed fixture-database-password"),
    );
    let close = context.error(
        "关闭连接",
        diagnostic::Writes::Database,
        &std::io::Error::other("close failed mysql://unknown:unlisted@localhost/db"),
    );
    let error = diagnostic::finish::<()>(Err(operation), Err(close))
        .unwrap_err()
        .to_string();
    for expected in [
        "业务迁移执行",
        "operation failed",
        "关闭连接",
        "close failed",
        "DDL 或账本可能已部分提交",
        "禁止自动重放",
    ] {
        assert!(error.contains(expected), "{error}");
    }
    assert!(!error.contains("fixture-database-password"));
    assert!(!error.contains("unknown:unlisted"));
    assert!(!error.contains("://"));
    let adoption = diagnostic::Context {
        command: "control adopt --apply",
        ..context
    };
    let error = adoption
        .error(
            "接管账本应用",
            diagnostic::Writes::Database,
            &std::io::Error::other("apply failed"),
        )
        .to_string();
    assert!(
        error
            .contains("cargo migrate -- --config-dir <当前已显示配置目录> control adopt --inspect")
    );
    assert!(!error.contains("cargo migrate -- control"));
    assert!(!error.contains("或 tenant-data"));
}
