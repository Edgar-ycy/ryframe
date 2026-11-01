//! 控制库与租户数据目标的独立迁移命令。
//!
//! 生产环境只有显式 scope 的 `ryframe-migrate ... up` 可以执行 DDL。API 和 Worker
//! 进程继续使用 `database.migration_mode = "verify"`，不会在启动时遍历独立目标。

use ryframe_config::{AppConfig, ConfigurationArgs, Environment, TenantDatabaseTargetKind};
use ryframe_sdk::{BusinessMigrationScope, RyFrameBusinessModule};
use ryframe_tenant_db::TenantDatabaseTargetRegistry;
use sea_orm::DatabaseConnection;
#[path = "ryframe_migrate/adoption.rs"]
mod adoption;
#[path = "ryframe_migrate/bootstrap.rs"]
mod bootstrap;
#[path = "ryframe_migrate/diagnostic.rs"]
mod diagnostic;
#[path = "ryframe_migrate/ownership.rs"]
mod ownership;
#[path = "ryframe_migrate/private_artifact.rs"]
mod private_artifact;
#[path = "ryframe_migrate/sql_export.rs"]
mod sql_export;
#[path = "ryframe_migrate/targets.rs"]
mod targets;

const USAGE: &str = "用法：\n  cargo migrate -- control <up|verify|status>\n  cargo migrate -- control export-sql --output <新路径> [--plan|--write]\n  cargo migrate -- tenant-data <up|verify|status> (--target <key>|--all)\n  cargo migrate -- <control|tenant-data> adopt --module <模块> [--target <键>] [--output .local-tests/adopt.json --write]\n  cargo migrate -- <control|tenant-data> adopt <--inspect|--apply> .local-tests/adopt.json [--target <键>] [--write]\n  cargo migrate -- control ownership-adopt [--output .local-tests/ownership.json --write]\n  cargo migrate -- tenant-data ownership-adopt --target <键> [--output .local-tests/ownership.json --write]\n  cargo migrate -- <control|tenant-data> ownership-adopt <--inspect|--apply> .local-tests/ownership.json [--target <键>] [--write]\n  接管支持 --plan：只显示任务图，不读配置、文件或数据库。";

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
    OwnershipAdopt(ownership::Options),
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
async fn main() -> std::process::ExitCode {
    match execute().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn execute() -> Result<(), diagnostic::Failure> {
    use diagnostic::{Context, Writes};
    let initial = Context {
        directory: std::path::Path::new("（配置目录尚未解析）"),
        environment: None,
        config: None,
        command: "参数与运行环境",
        target: None,
    };
    ryframe::crypto::install_crypto_provider()
        .map_err(|error| initial.error("加密提供者初始化", Writes::None, &error))?;
    let arguments = ConfigurationArgs::parse(std::env::args().skip(1).collect())
        .map_err(|error| initial.error("配置参数解析", Writes::None, &error))?;
    let initial = Context {
        directory: &arguments.directory,
        ..initial
    };
    let command = parse_command(arguments.arguments)
        .map_err(|error| initial.error("命令解析", Writes::None, error.as_ref()))?;
    let (scope, operation) = match command {
        Command::Migrate { scope, operation } => (scope, operation),
        Command::ExportSql { output, write } => {
            return sql_export::run(&arguments.directory, &output, write);
        }
        Command::Adopt(options) => return adoption::run(&arguments.directory, options).await,
        Command::OwnershipAdopt(options) => {
            return ownership::run(&arguments.directory, options).await;
        }
    };
    let command = format!(
        "{} {}",
        match &scope {
            MigrationScope::Control => "control",
            MigrationScope::TenantData(_) => "tenant-data",
        },
        operation.name()
    );
    let selected_target = match &scope {
        MigrationScope::TenantData(TargetSelection::One(target)) => Some(target.clone()),
        _ => None,
    };
    let initial = Context {
        command: &command,
        target: selected_target.as_deref(),
        ..initial
    };
    let environment = Environment::from_env().map_err(|_| {
        initial.error(
            "APP_ENV 选择",
            Writes::None,
            &std::io::Error::other("APP_ENV 必须为 dev、test 或 prod"),
        )
    })?;
    let initial = Context {
        environment: Some(environment),
        ..initial
    };
    let config = AppConfig::load(&arguments.directory, environment)
        .map_err(|error| initial.error("配置加载", Writes::None, &error))?;
    let context = Context {
        config: Some(&config),
        ..initial
    };
    let modules = ryframe::business::registered_business_modules()
        .map_err(|error| context.error("业务模块登记", Writes::None, &error))?;
    match scope {
        MigrationScope::Control => run_control(operation, &config, &modules, &context).await,
        MigrationScope::TenantData(selection) => {
            run_tenant_data(operation, selection, &config, &modules, &context).await
        }
    }
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Verify => "verify",
            Self::Status => "status",
        }
    }
    fn writes(self) -> diagnostic::Writes {
        if matches!(self, Self::Up) {
            diagnostic::Writes::Database
        } else {
            diagnostic::Writes::None
        }
    }
}

fn parse_command(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    if args.get(1).is_some_and(|value| value == "ownership-adopt") {
        return ownership::parse(&args).map(Command::OwnershipAdopt);
    }
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
    context: &diagnostic::Context<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let hashes = if matches!(operation, Operation::Up) {
        bootstrap::load_hashes(config).map_err(|error| {
            context.error("初始化密码哈希", diagnostic::Writes::None, error.as_ref())
        })?
    } else {
        None
    };
    let database = ryframe_db::connection::connect_with_sql_logging(
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await
    .map_err(|error| context.error("连接池建立", diagnostic::Writes::None, &error))?;
    let mut phase = match operation {
        Operation::Up => "控制库框架迁移",
        Operation::Verify => "控制库框架账本/结构核验",
        Operation::Status => "控制库迁移状态读取",
    };
    let result = async {
        match operation {
            Operation::Up => {
                ownership::control_up(
                    &database,
                    config.scope_id.as_str(),
                    hashes.as_ref(),
                    business_modules,
                )
                .await?;
                return Ok(());
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
        phase = match operation {
            Operation::Up => "业务迁移与最终结构核验",
            Operation::Verify => "业务账本与结构只读核验",
            Operation::Status => "业务账本预检与状态读取",
        };
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
    diagnostic::finish(
        result.map_err(|error| context.error(phase, operation.writes(), &error)),
        close_result.map_err(|error| context.error("关闭连接", operation.writes(), &error)),
    )
}

async fn run_tenant_data(
    operation: Operation,
    selection: TargetSelection,
    config: &AppConfig,
    business_modules: &[RyFrameBusinessModule],
    context: &diagnostic::Context<'_>,
) -> Result<(), diagnostic::Failure> {
    let registry = TenantDatabaseTargetRegistry::new(
        &config.tenant_data,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .map_err(|error| context.error("目标登记", diagnostic::Writes::None, &error))?;
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
        context,
    )
    .await
    .map_err(|error| {
        context.error(
            "所选目标连接/物理身份预检",
            diagnostic::Writes::None,
            error.as_ref(),
        )
    })?;
    let targets = targets::unique_targets(&registry, &identities).await?;
    let mut previous_writes = false;
    for target in targets {
        let report = diagnostic::Context {
            target: Some(&target),
            ..*context
        };
        let expected = identities.get(&target).ok_or("迁移预检遗漏目标身份")?;
        let aliases = identities
            .iter()
            .filter(|(_, identity)| *identity == expected)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        run_target(
            operation,
            &target,
            (expected, &aliases),
            &registry,
            business_modules,
            &report,
            previous_writes,
        )
        .await?;
        previous_writes |= matches!(operation, Operation::Up);
    }
    Ok(())
}

async fn run_target(
    operation: Operation,
    target: &str,
    binding: (&ryframe_db::connection::DatabaseIdentity, &[String]),
    registry: &TenantDatabaseTargetRegistry,
    modules: &[RyFrameBusinessModule],
    context: &diagnostic::Context<'_>,
    previous_writes: bool,
) -> Result<(), diagnostic::Failure> {
    let (expected, aliases) = binding;
    let config = context.config.ok_or("迁移上下文缺少配置")?;
    let prior = if previous_writes {
        diagnostic::Writes::Database
    } else {
        diagnostic::Writes::None
    };
    let mut phase = "目标物理身份复核";
    match registry.target_kind(target) {
        Some(TenantDatabaseTargetKind::Control) => {
            let database = ryframe_db::connection::connect_with_sql_logging(
                &config.database.primary,
                config.database.sql_log_level,
                config.database.sql_slow_threshold_ms,
            )
            .await
            .map_err(|error| context.error("连接池建立", prior, &error))?;
            let mut writes = prior;
            let result = async {
                targets::verify_identity(&database, target, expected).await?;
                if matches!(operation, Operation::Up) {
                    writes = diagnostic::Writes::Database;
                }
                run_tenant_data_operation(
                    operation,
                    target,
                    &database,
                    false,
                    modules,
                    &mut phase,
                    (config.scope_id.as_str(), aliases),
                )
                .await?;
                Ok::<(), diagnostic::Failure>(())
            }
            .await;
            let closed = database.close().await;
            diagnostic::finish(
                result.map_err(|error| context.error(phase, writes, error.as_ref())),
                closed.map_err(|error| context.error("关闭连接", writes, &error)),
            )
        }
        Some(TenantDatabaseTargetKind::Mysql) => {
            let lease = registry.acquire(target).await.map_err(|error| {
                context.error("独立目标连接池获取（registry 不可用）", prior, &error)
            })?;
            targets::verify_identity(lease.connection(), target, expected)
                .await
                .map_err(|error| context.error(phase, prior, error.as_ref()))?;
            run_tenant_data_operation(
                operation,
                target,
                lease.connection(),
                true,
                modules,
                &mut phase,
                (config.scope_id.as_str(), aliases),
            )
            .await
            .map_err(|error| context.error(phase, operation.writes(), &error))?;
            drop(lease);
            Ok(())
        }
        None => Err(context.error("目标选择", prior, &std::io::Error::other("目标未登记"))),
    }
}

async fn run_tenant_data_operation(
    operation: Operation,
    target: &str,
    database: &DatabaseConnection,
    mysql_target: bool,
    business_modules: &[RyFrameBusinessModule],
    phase: &mut &'static str,
    binding: (&str, &[String]),
) -> Result<(), sea_orm::DbErr> {
    let (scope_id, aliases) = binding;
    *phase = "租户框架/边界只读核验";
    match operation {
        Operation::Up => {
            return ownership::tenant_up(
                database,
                scope_id,
                target,
                mysql_target,
                business_modules,
                aliases,
            )
            .await;
        }
        Operation::Verify => {
            if mysql_target {
                ryframe_tenant_db::migration::verify_mysql_target(database).await?;
            } else {
                ryframe_tenant_db::migration::verify_shared_control(database).await?;
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
    *phase = "租户业务账本与结构核验";
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
