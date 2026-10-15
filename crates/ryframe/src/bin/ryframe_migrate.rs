//! 控制库与租户数据目标的独立迁移命令。
//!
//! 生产环境只有显式 scope 的 `ryframe-migrate ... up` 可以执行 DDL。API 和 Worker
//! 进程继续使用 `database.migration_mode = "verify"`，不会在启动时遍历独立目标。

use ryframe_config::{AppConfig, Environment, TenantDatabaseTargetKind};
use ryframe_sdk::{BusinessMigrationScope, RyFrameBusinessModule};
use ryframe_tenant_db::TenantDatabaseTargetRegistry;
use sea_orm::DatabaseConnection;
#[path = "ryframe_migrate/adoption.rs"]
mod adoption;
#[path = "ryframe_migrate/bootstrap.rs"]
mod bootstrap;
#[path = "ryframe_migrate/private_artifact.rs"]
mod private_artifact;
#[path = "ryframe_migrate/sql_export.rs"]
mod sql_export;
#[path = "ryframe_migrate/targets.rs"]
mod targets;

const USAGE: &str = "用法：\n  cargo migrate -- control <up|verify|status>\n  cargo migrate -- tenant-data <up|verify|status> (--target <key>|--all)\n  cargo migrate -- <control|tenant-data> adopt --module <模块> [--target <键>] [--output .local-tests/adopt.json --write]\n  cargo migrate -- <control|tenant-data> adopt <--inspect|--apply> .local-tests/adopt.json [--target <键>] [--write]\n  接管支持 --plan：只显示任务图，不读配置、文件或数据库。";

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

enum Command {
    Adopt(adoption::Options),
    Migrate {
        scope: MigrationScope,
        operation: Operation,
    },
    ExportSql {
        output: std::path::PathBuf,
        write: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ryframe::crypto::install_crypto_provider()?;
    let command = parse_command(std::env::args().skip(1).collect())?;

    let (scope, operation) = match command {
        Command::Migrate { scope, operation } => (scope, operation),
        Command::ExportSql { output, write } => return sql_export::run(&output, write),
        Command::Adopt(options) => return adoption::run(options).await,
    };
    let environment = Environment::from_env()?;
    let config = AppConfig::load_from_env(environment)?;
    let business_modules = ryframe::business::registered_business_modules()?;
    match scope {
        MigrationScope::Control => run_control(operation, &config, &business_modules).await?,
        MigrationScope::TenantData(selection) => {
            run_tenant_data(operation, selection, &config, &business_modules).await?
        }
    }
    Ok(())
}

fn parse_command(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    if args.get(1).is_some_and(|value| value == "adopt") {
        return adoption::parse(&args).map(Command::Adopt);
    }
    if args.first().is_some_and(|value| value == "control")
        && args.get(1).is_some_and(|value| value == "export-sql")
    {
        return sql_export::parse(&args[2..]);
    }
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
    Ok(Command::Migrate { scope, operation })
}

async fn run_control(
    operation: Operation,
    config: &AppConfig,
    business_modules: &[RyFrameBusinessModule],
) -> Result<(), Box<dyn std::error::Error>> {
    let hashes = if matches!(operation, Operation::Up) {
        bootstrap::load_hashes()?
    } else {
        None
    };
    let database = ryframe_db::connection::connect_with_sql_logging(
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await?;
    let result = async {
        match operation {
            Operation::Up => {
                ryframe_db::migration::up(&database, hashes.as_ref()).await?;
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

    let identities = targets::preflight(
        &registry,
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
        &targets,
    )
    .await?;
    for target in targets {
        let expected = identities.get(&target).ok_or("迁移预检遗漏目标身份")?;
        match registry.target_kind(&target) {
            Some(TenantDatabaseTargetKind::Control) => {
                let database = ryframe_db::connection::connect_with_sql_logging(
                    &config.database.primary,
                    config.database.sql_log_level,
                    config.database.sql_slow_threshold_ms,
                )
                .await?;
                let result = async {
                    targets::verify_identity(&database, &target, expected).await?;
                    run_tenant_data_operation(
                        operation,
                        &target,
                        &database,
                        false,
                        business_modules,
                    )
                    .await?;
                    Ok::<(), Box<dyn std::error::Error>>(())
                }
                .await;
                let close_result = database.close().await;
                result?;
                close_result?;
            }
            Some(TenantDatabaseTargetKind::Mysql) => {
                let lease = registry.acquire(&target).await?;
                targets::verify_identity(lease.connection(), &target, expected).await?;
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
            ryframe_tenant_db::migration::up_pending_business_migrations(database).await?;
            run_business_migrations(
                operation,
                BusinessMigrationScope::Tenant,
                database,
                business_modules,
                &format!("tenant-data/{target}"),
            )
            .await?;
            if mysql_target {
                ryframe_tenant_db::migration::verify_mysql_target(database).await?;
            } else {
                ryframe_tenant_db::migration::verify(database).await?;
            }
            println!("scope=tenant-data target={target} migration=completed schema=verified");
            return Ok(());
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
    let migrations = modules
        .iter()
        .flat_map(|module| module.migrations().iter())
        .filter(|migration| migration.scope() == scope)
        .map(|migration| migration.as_ref())
        .collect::<Vec<_>>();
    if matches!(operation, Operation::Verify) {
        return ryframe_sdk::verify_business_migrations(database, scope, &migrations)
            .await
            .map_err(|error| {
                sea_orm::DbErr::Custom(format!("目标 {target} 业务迁移结构校验失败：{error}"))
            });
    }
    ryframe_sdk::validate_business_migration_ledger(database, scope, &migrations)
        .await
        .map_err(|error| {
            sea_orm::DbErr::Custom(format!(
                "目标 {target} 业务迁移账本预检失败，尚未执行业务 DDL：{error}"
            ))
        })?;
    for module in modules {
        for migration in module
            .migrations()
            .iter()
            .filter(|migration| migration.scope() == scope)
        {
            let result = match operation {
                Operation::Up => migration.up(database).await,
                Operation::Verify => unreachable!("整组结构校验已在循环前执行"),
                Operation::Status => {
                    let status = migration.status(database).await.map_err(|error| {
                        sea_orm::DbErr::Custom(format!(
                            "目标 {target} 的业务迁移 {}/{} 失败：{error}",
                            module.name(),
                            migration.id()
                        ))
                    })?;
                    println!(
                        "scope={target} module={} migration={} applied={} expected={} current={} origin={} missing={}",
                        module.name(),
                        migration.id(),
                        status.applied,
                        status.expected,
                        status.is_current(),
                        status.origin.map_or("pending", |origin| origin.as_str()),
                        display_versions(&status.missing),
                    );
                    Ok(())
                }
            };
            result.map_err(|error| {
                sea_orm::DbErr::Custom(format!(
                    "目标 {target} 的业务迁移 {}/{} 失败：{error}",
                    module.name(),
                    migration.id()
                ))
            })?;
        }
    }
    if matches!(operation, Operation::Up) {
        ryframe_sdk::verify_business_migrations(database, scope, &migrations)
            .await
            .map_err(|error| {
                sea_orm::DbErr::Custom(format!(
                    "目标 {target} 业务迁移已执行但最终结构校验失败：{error}"
                ))
            })?;
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
