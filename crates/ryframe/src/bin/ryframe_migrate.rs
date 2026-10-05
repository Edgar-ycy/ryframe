//! 控制库与租户数据目标的独立迁移命令。
//!
//! 生产环境只有显式 scope 的 `ryframe-migrate ... up` 可以执行 DDL。API 和 Worker
//! 进程继续使用 `database.migration_mode = "verify"`，不会在启动时遍历独立目标。

use ryframe_config::{AppConfig, Environment, TenantDatabaseTargetKind};
use ryframe_sdk::{BusinessMigrationScope, RyFrameBusinessModule};
use ryframe_tenant_db::TenantDatabaseTargetRegistry;
use sea_orm::DatabaseConnection;

const USAGE: &str = "usage:\n  ryframe-migrate control <up|verify|status>\n  ryframe-migrate tenant-data <up|verify|status> (--target <key>|--all)";

#[derive(Clone, Copy)]
enum Operation {
    Up,
    Verify,
    Status,
}

enum MigrationScope {
    Control,
    TenantData(TargetSelection),
}

enum TargetSelection {
    One(String),
    All,
}

struct Command {
    scope: MigrationScope,
    operation: Operation,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ryframe::crypto::install_crypto_provider()?;
    let command = parse_command(std::env::args().skip(1).collect())?;

    let environment = Environment::from_env()?;
    let config = AppConfig::load_from_env(environment)?;
    let business_modules = ryframe::business::registered_business_modules()?;
    match command.scope {
        MigrationScope::Control => {
            run_control(command.operation, &config, &business_modules).await?
        }
        MigrationScope::TenantData(selection) => {
            run_tenant_data(command.operation, selection, &config, &business_modules).await?
        }
    }
    Ok(())
}

fn parse_command(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    let [scope, operation, rest @ ..] = args.as_slice() else {
        return Err(USAGE.into());
    };
    let operation = match operation.as_str() {
        "up" => Operation::Up,
        "verify" => Operation::Verify,
        "status" => Operation::Status,
        _ => return Err(USAGE.into()),
    };
    let scope = match (scope.as_str(), rest) {
        ("control", []) => MigrationScope::Control,
        ("tenant-data", [flag]) if flag == "--all" => {
            MigrationScope::TenantData(TargetSelection::All)
        }
        ("tenant-data", [flag, target]) if flag == "--target" && !target.trim().is_empty() => {
            MigrationScope::TenantData(TargetSelection::One(target.clone()))
        }
        _ => return Err(USAGE.into()),
    };
    Ok(Command { scope, operation })
}

async fn run_control(
    operation: Operation,
    config: &AppConfig,
    business_modules: &[RyFrameBusinessModule],
) -> Result<(), Box<dyn std::error::Error>> {
    let database = ryframe_db::connection::connect_with_sql_logging(
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await?;
    let result = async {
        match operation {
            Operation::Up => {
                ryframe_db::migration::up(&database).await?;
                println!("scope=control migration=completed schema=verified");
            }
            Operation::Verify => {
                ryframe_db::migration::verify(&database).await?;
                println!("scope=control migration_ledger=current schema=verified");
            }
            Operation::Status => {
                let status = ryframe_db::migration::status(&database).await?;
                print_control_status("control", &status);
            }
        }
        run_business_migrations(
            operation,
            BusinessMigrationScope::Control,
            &database,
            business_modules,
            "control",
        )
        .await?;
        Ok::<(), sea_orm::DbErr>(())
    }
    .await;
    let close_result = database.close().await;
    result?;
    close_result?;
    Ok(())
}

async fn run_tenant_data(
    operation: Operation,
    selection: TargetSelection,
    config: &AppConfig,
    business_modules: &[RyFrameBusinessModule],
) -> Result<(), Box<dyn std::error::Error>> {
    let registry = TenantDatabaseTargetRegistry::new(
        &config.tenant_data,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )?;
    let targets = match selection {
        TargetSelection::One(target) => vec![target],
        TargetSelection::All => registry.target_keys(),
    };
    if targets.is_empty() {
        println!("scope=tenant-data targets=0 result=no-configured-targets");
        return Ok(());
    }

    for target in targets {
        match registry.target_kind(&target) {
            Some(TenantDatabaseTargetKind::Control) => {
                let database = ryframe_db::connection::connect_with_sql_logging(
                    &config.database.primary,
                    config.database.sql_log_level,
                    config.database.sql_slow_threshold_ms,
                )
                .await?;
                let result = run_tenant_data_operation(
                    operation,
                    &target,
                    &database,
                    false,
                    business_modules,
                )
                .await;
                let close_result = database.close().await;
                result?;
                close_result?;
            }
            Some(TenantDatabaseTargetKind::Mysql) => {
                let lease = registry.acquire(&target).await?;
                run_tenant_data_operation(
                    operation,
                    &target,
                    lease.connection(),
                    true,
                    business_modules,
                )
                .await?;
                drop(lease);
            }
            None => {
                return Err(format!("unknown tenant-data target: {target}").into());
            }
        }
    }
    Ok(())
}

async fn run_tenant_data_operation(
    operation: Operation,
    target: &str,
    database: &DatabaseConnection,
    mysql_target: bool,
    business_modules: &[RyFrameBusinessModule],
) -> Result<(), sea_orm::DbErr> {
    match operation {
        Operation::Up => {
            if mysql_target {
                ryframe_tenant_db::migration::ensure_mysql_target_boundary(database).await?;
            }
            ryframe_tenant_db::migration::up(database).await?;
            if mysql_target {
                ryframe_tenant_db::migration::verify_mysql_target(database).await?;
            }
            println!("scope=tenant-data target={target} migration=completed schema=verified");
        }
        Operation::Verify => {
            if mysql_target {
                ryframe_tenant_db::migration::verify_mysql_target(database).await?;
            } else {
                ryframe_tenant_db::migration::verify(database).await?;
            }
            println!("scope=tenant-data target={target} migration_ledger=current schema=verified");
        }
        Operation::Status => {
            if mysql_target {
                ryframe_tenant_db::migration::ensure_mysql_target_boundary(database).await?;
            }
            let status = ryframe_tenant_db::migration::status(database).await?;
            print_tenant_data_status(&format!("tenant-data/{target}"), &status);
        }
    }
    run_business_migrations(
        operation,
        BusinessMigrationScope::Tenant,
        database,
        business_modules,
        &format!("tenant-data/{target}"),
    )
    .await?;
    Ok(())
}

async fn run_business_migrations(
    operation: Operation,
    scope: BusinessMigrationScope,
    database: &DatabaseConnection,
    modules: &[RyFrameBusinessModule],
    target: &str,
) -> Result<(), sea_orm::DbErr> {
    for module in modules {
        for migration in module
            .migrations()
            .iter()
            .filter(|migration| migration.scope() == scope)
        {
            let result = match operation {
                Operation::Up => migration.up(database).await,
                Operation::Verify => migration.verify(database).await,
                Operation::Status => {
                    let status = migration.status(database).await.map_err(|error| {
                        sea_orm::DbErr::Custom(format!(
                            "business migration {} for {target} failed: {error}",
                            module.name()
                        ))
                    })?;
                    println!(
                        "scope={target} module={} applied={} expected={} current={} missing={}",
                        module.name(),
                        status.applied,
                        status.expected,
                        status.is_current(),
                        display_versions(&status.missing),
                    );
                    Ok(())
                }
            };
            result.map_err(|error| {
                sea_orm::DbErr::Custom(format!(
                    "business migration {} for {target} failed: {error}",
                    module.name()
                ))
            })?;
        }
    }
    Ok(())
}

fn print_control_status(scope: &str, status: &ryframe_db::migration::MigrationStatus) {
    println!(
        "scope={scope} applied={} expected={} up_to_date={} missing={} unexpected={}",
        status.applied,
        status.expected,
        status.is_up_to_date(),
        display_versions(&status.missing),
        display_versions(&status.unexpected)
    );
}

fn print_tenant_data_status(scope: &str, status: &ryframe_tenant_db::migration::MigrationStatus) {
    println!(
        "scope={scope} applied={} expected={} up_to_date={} schema_fingerprint={} missing={} unexpected={}",
        status.applied,
        status.expected,
        status.is_up_to_date(),
        status.schema_fingerprint,
        display_versions(&status.missing),
        display_versions(&status.unexpected)
    );
}

fn display_versions(versions: &[String]) -> String {
    if versions.is_empty() {
        "-".into()
    } else {
        versions.join(",")
    }
}
