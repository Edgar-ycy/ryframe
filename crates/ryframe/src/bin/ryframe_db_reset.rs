//! Explicit, non-production MySQL reset utility.
//!
//! Example:
//! `cargo run -p ryframe --bin ryframe-db-reset -- --database ryframe_config --confirm-reset RESET-RYFRAME-DATABASE`

use ryframe_config::DbConnection;
use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};

const CONFIRMATION: &str = "RESET-RYFRAME-DATABASE";
const ADMIN_PASSWORD: &str = "123456";
const USER_PASSWORD: &str = "123456";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let environment = std::env::var("APP_ENV")
        .map_err(|_| "refusing reset: APP_ENV must be explicitly set to dev or test")?;
    let normalized_environment = match environment.trim().to_ascii_lowercase().as_str() {
        "dev" | "development" => "dev",
        "test" | "testing" => "test",
        "prod" | "production" => {
            return Err("database reset is permanently disabled in production".into());
        }
        _ => return Err("refusing reset: APP_ENV must be explicitly set to dev or test".into()),
    };

    let args = parse_args()?;
    let config = ryframe_config::AppConfig::load("config")?;
    if config.database.primary.database != args.expected_database {
        return Err(format!(
            "configured database does not match --database (configured: {}, expected: {})",
            config.database.primary.database, args.expected_database
        )
        .into());
    }
    validate_database_name(&args.expected_database)
        .map_err(|error| format!("refusing reset: {error}"))?;

    // Do all fallible password work before the first destructive operation.
    let admin_hash = ryframe_auth::password::hash(ADMIN_PASSWORD)?;
    let user_hash = ryframe_auth::password::hash(USER_PASSWORD)?;

    println!(
        "Recreating MySQL database '{}' on {}:{} in '{}' environment",
        args.expected_database,
        config.database.primary.host,
        config.database.primary.port,
        normalized_environment
    );

    recreate_database(&config.database.primary, &args.expected_database).await?;

    let database = ryframe_db::connection::connect(&config.database.primary).await?;
    verify_connected_database(&database, &args.expected_database).await?;
    let initialize_result: Result<(), Box<dyn std::error::Error>> = async {
        ryframe_db_migration::run(&database).await?;
        update_password(&database, "admin", admin_hash).await?;
        update_password(&database, "user", user_hash).await?;
        Ok(())
    }
    .await;
    let close_result = database.close().await;
    initialize_result?;
    close_result?;

    println!("Database reset completed successfully");
    Ok(())
}

async fn recreate_database(
    connection: &DbConnection,
    expected_database: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let [drop_statement, create_statement] = recreate_database_statements(expected_database)
        .map_err(|error| format!("refusing reset: {error}"))?;

    // Connect through MySQL's administration schema so the target database
    // can be dropped even when it is missing or contains an incompatible
    // legacy schema. This connection never includes the target database.
    let mut admin_config = connection.clone();
    admin_config.database = "mysql".to_owned();
    admin_config.max_connections = 1;
    admin_config.min_connections = 1;
    let admin = ryframe_db::connection::connect(&admin_config).await?;
    verify_connected_database(&admin, "mysql").await?;

    let recreate_result: Result<(), sea_orm::DbErr> = async {
        admin.execute_unprepared(&drop_statement).await?;
        admin.execute_unprepared(&create_statement).await?;
        Ok(())
    }
    .await;
    let close_result = admin.close().await;
    recreate_result?;
    close_result?;
    Ok(())
}

fn recreate_database_statements(database: &str) -> Result<[String; 2], String> {
    validate_database_name(database)?;
    Ok([
        format!("DROP DATABASE IF EXISTS `{database}`"),
        format!("CREATE DATABASE `{database}` CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci"),
    ])
}

fn validate_database_name(database: &str) -> Result<(), String> {
    if database.is_empty() || database.len() > 64 {
        return Err("database name must contain 1-64 ASCII characters".into());
    }
    if !database
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("database name may contain only ASCII letters, digits, and `_`".into());
    }
    if ["mysql", "information_schema", "performance_schema", "sys"]
        .iter()
        .any(|reserved| database.eq_ignore_ascii_case(reserved))
    {
        return Err(format!("system database `{database}` cannot be reset"));
    }
    Ok(())
}

async fn verify_connected_database(
    database: &sea_orm::DatabaseConnection,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT DATABASE()".to_owned(),
        ))
        .await?
        .ok_or("database identity query returned no row")?;
    let actual = String::try_get_by_index(&row, 0)
        .map_err(|error| format!("cannot read connected database name: {error:?}"))?;
    if actual != expected {
        return Err(format!(
            "connected database does not match --database (connected: {actual}, expected: {expected})"
        )
        .into());
    }
    Ok(())
}

struct ResetArgs {
    expected_database: String,
}

fn parse_args() -> Result<ResetArgs, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut expected_database = None;
    let mut confirmation = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--database" => expected_database = args.next(),
            "--confirm-reset" => confirmation = args.next(),
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    if confirmation.as_deref() != Some(CONFIRMATION) {
        return Err(format!("refusing reset: pass --confirm-reset {CONFIRMATION}").into());
    }
    let expected_database = expected_database
        .filter(|value| !value.trim().is_empty())
        .ok_or("refusing reset: --database <expected-name> is required")?;
    Ok(ResetArgs { expected_database })
}

async fn update_password(
    database: &sea_orm::DatabaseConnection,
    username: &str,
    password_hash: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "UPDATE `sys_user` SET `password_hash` = ?, `status` = '1', `auth_version` = `auth_version` + 1 WHERE `tenant_id` = 'system' AND `username` = ?",
            [password_hash.into(), username.into()],
        ))
        .await?;
    if result.rows_affected() != 1 {
        return Err(format!(
            "expected exactly one system user named '{username}', updated {}",
            result.rows_affected()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{recreate_database_statements, validate_database_name};

    #[test]
    fn reset_database_name_is_strictly_validated() {
        for valid in ["ryframe_config", "ryframe_dev", "test123"] {
            assert!(validate_database_name(valid).is_ok(), "rejected {valid}");
        }
        for invalid in [
            "",
            "mysql",
            "INFORMATION_SCHEMA",
            "performance_schema",
            "sys",
            "ryframe-config",
            "ryframe config",
            "ryframe`; DROP DATABASE mysql; --",
        ] {
            assert!(
                validate_database_name(invalid).is_err(),
                "accepted {invalid}"
            );
        }
        assert!(validate_database_name(&"a".repeat(65)).is_err());
    }

    #[test]
    fn reset_recreates_the_exact_confirmed_database() {
        assert_eq!(
            recreate_database_statements("ryframe_config").unwrap(),
            [
                "DROP DATABASE IF EXISTS `ryframe_config`",
                "CREATE DATABASE `ryframe_config` CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci",
            ]
        );
    }
}
