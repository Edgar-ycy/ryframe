use super::*;
use ryframe_config::{SqlLogLevel, TenantDataConfig, TenantDatabaseTargetConfig};
use ryframe_tenant_db::TenantDatabaseTargetRegistry;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aliases_of_one_physical_database_fail_before_any_migration() {
    mysql::run_mysql_test("migrationalias", |database| async move {
        let identity = ryframe_db::connection::physical_identity(&database)
            .await
            .map_err(mysql::db_error)?;
        let configured =
            mysql::MySqlTestConfiguration::load().unwrap_or_else(|error| panic!("{error}"));
        let primary = configured
            .connection(&identity.database, None)
            .unwrap_or_else(|error| panic!("{error}"));
        let first = target("first", &primary);
        let mut second = target("second", &primary);
        if primary.host == "127.0.0.1" {
            second.host = Some("localhost".into());
        }
        let config = TenantDataConfig {
            targets: vec![first, second],
            ..TenantDataConfig::default()
        };
        let registry =
            TenantDatabaseTargetRegistry::new(&config, SqlLogLevel::Off, 200).map_err(message)?;
        let result = migration_targets::preflight(
            &registry,
            &primary,
            SqlLogLevel::Off,
            200,
            &["first".into(), "second".into()],
        )
        .await;
        let plan = migration_targets::preflight(
            &registry,
            &primary,
            SqlLogLevel::Off,
            200,
            &["shared-control".into()],
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

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
