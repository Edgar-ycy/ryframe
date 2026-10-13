use crate::business::{
    BusinessBootstrapOptions, BusinessGenerateOptions, bootstrap_business_package,
    generate_business_package, locate_business_package, workspace_root,
};

#[cfg(feature = "schema-import")]
use crate::import::{inspect_existing_table, rust_model_source};
#[cfg(feature = "schema-import")]
use ryframe_config::{AppConfig, Environment};
#[cfg(feature = "schema-import")]
use ryframe_tenant_db::TenantDatabaseTargetRegistry;

const USAGE: &str = "用法：\n  cargo generate -- new-business <模块名或业务包名> [--plan|--write]\n  cargo generate -- resource [--package <crate>] [--model <Model>] [--plan|--write] [--sync-frontend]\n  cargo generate --features schema-import -- import [--connection <name>] [--database <control|tenant>] [--package <crate>] --table <table> [--plan|--write]\n\n默认预览，仅 --write 更新源码；默认值读取 config/tools.toml 与 .local-tests/tools.toml。";

/// 执行命令行与 Rust 入口共用的生成流程。
pub fn run_from_args(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("--help" | "-h" | "help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some("new-business") => new_business(&args[1..]),
        Some("resource") => resource(&args[1..]),
        Some("import") => run_import(&args[1..]),
        Some(command) => Err(format!("未知生成操作：{command}\n{USAGE}")),
    }
}

#[cfg(feature = "schema-import")]
fn run_import(args: &[String]) -> Result<(), String> {
    let parsed = Arguments::parse_import(args)?;
    if parsed.flag("--plan") {
        print_plan("import", &parsed)?;
        return Ok(());
    }
    tokio::runtime::Runtime::new()
        .map_err(|error| format!("无法创建导入运行时：{error}"))?
        .block_on(import(args))
}
#[cfg(not(feature = "schema-import"))]
fn run_import(args: &[String]) -> Result<(), String> {
    let parsed = Arguments::parse_import(args)?;
    if parsed.flag("--plan") {
        return print_plan("import", &parsed);
    }
    Err("数据库导入需要运行 cargo generate --features schema-import -- import ...".into())
}

fn new_business(args: &[String]) -> Result<(), String> {
    let [module, rest @ ..] = args else {
        return Err(USAGE.into());
    };
    let module = module.strip_suffix("-business").unwrap_or(module);
    let parsed = Arguments::parse_new(rest)?;
    crate::business::validate_module(module).map_err(|error| error.to_string())?;
    if parsed.flag("--plan") {
        println!(
            "生成计划：创建 {module}-business → 注册 Workspace、SDK、进程 feature 与模块；不生成 CRUD、不执行任务。"
        );
        return Ok(());
    }
    let write = parsed.flag("--write");
    let current_dir =
        std::env::current_dir().map_err(|error| format!("无法读取当前目录：{error}"))?;
    let workspace_root = workspace_root(&current_dir).map_err(|error| error.to_string())?;
    let report = bootstrap_business_package(BusinessBootstrapOptions {
        workspace_root: &workspace_root,
        module,
        write,
    })
    .map_err(|error| error.to_string())?;
    for path in &report.created {
        println!("create {path}");
    }
    for path in &report.updated {
        println!("update {path}");
    }
    println!(
        "业务 crate {}：新增 {}，更新 {}。",
        if write { "创建完成" } else { "预览" },
        report.created.len(),
        report.updated.len(),
    );
    Ok(())
}

fn resource(args: &[String]) -> Result<(), String> {
    let parsed = Arguments::parse(args)?;
    if parsed.flag("--plan") {
        return print_plan("resource", &parsed);
    }
    let (_, settings) = load_tools()?;
    let package = parsed
        .value("--package")
        .or(settings.generator.package.as_deref())
        .ok_or("缺少业务包：请传入 --package 或配置 generator.package")?;
    let sync = parsed.flag("--sync-frontend") || settings.generator.sync_frontend;
    if sync && !parsed.flag("--write") {
        return Err("同步前端必须显式提供 --write".into());
    }
    let current_dir =
        std::env::current_dir().map_err(|error| format!("无法读取当前目录：{error}"))?;
    let workspace_root = locate_business_package(&current_dir, package)
        .map_err(|error| error.to_string())?
        .workspace_root;
    let report = generate_business_package(BusinessGenerateOptions {
        current_dir: &current_dir,
        package,
        model: parsed.value("--model"),
        write: parsed.flag("--write"),
    })
    .map_err(|error| error.to_string())?;
    for (kind, paths) in [
        ("create", &report.created),
        ("update", &report.updated),
        ("remove", &report.removed),
    ] {
        for path in paths {
            println!("{kind} {path}");
        }
    }
    println!(
        "生成{}：新增 {}，更新 {}，删除 {}，未变化 {}。",
        if parsed.flag("--write") {
            "完成"
        } else {
            "预览"
        },
        report.created.len(),
        report.updated.len(),
        report.removed.len(),
        report.unchanged.len(),
    );
    if sync {
        sync_frontend(&workspace_root, &settings)?;
    }
    Ok(())
}

#[cfg(feature = "schema-import")]
async fn import(args: &[String]) -> Result<(), String> {
    let parsed = Arguments::parse_import(args)?;
    let (_, settings) = load_tools()?;
    let package = parsed
        .value("--package")
        .or(settings.generator.package.as_deref())
        .ok_or("缺少业务包：请传入 --package 或配置 generator.package")?;
    let scope = parsed
        .value("--database")
        .or(settings.generator.storage.as_deref())
        .ok_or("缺少目标类型：请传入 --database 或配置 generator.storage")?;
    let current = std::env::current_dir().map_err(|error| format!("无法读取当前目录：{error}"))?;
    let target = locate_business_package(&current, package).map_err(|error| error.to_string())?;
    let config =
        AppConfig::load_from_env(Environment::from_env().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let table = parsed.required("--table")?;
    let source = parsed
        .value("--connection")
        .or(settings.generator.source.as_deref())
        .unwrap_or("local");
    let model = match scope {
        "control" => {
            let connection = control_connection(&config, source).await?;
            let table_info = inspect_existing_table(&connection, table)
                .await
                .map_err(|error| error.to_string())?;
            connection
                .close()
                .await
                .map_err(|error| error.to_string())?;
            rust_model_source(&table_info, scope)?
        }
        "tenant" => {
            let registry = TenantDatabaseTargetRegistry::new(
                &config.tenant_data,
                config.database.sql_log_level,
                config.database.sql_slow_threshold_ms,
            )
            .map_err(|error| error.to_string())?;
            let key = if source == "local" {
                &config.tenant_data.default_target
            } else {
                source
            };
            let lease = registry
                .acquire(key)
                .await
                .map_err(|error| error.to_string())?;
            let table_info = inspect_existing_table(lease.connection(), table)
                .await
                .map_err(|error| error.to_string())?;
            rust_model_source(&table_info, scope)?
        }
        _ => return Err("--database 只支持 control 或 tenant".into()),
    };
    let file = target
        .root
        .join("src/resources")
        .join(format!("{}.rs", snake_case(table)?));
    if parsed.flag("--write") {
        std::fs::create_dir_all(file.parent().expect("资源目录有父路径"))
            .map_err(|error| error.to_string())?;
        if file.exists() {
            return Err(format!("{} 已存在；导入不会覆盖手写资源", file.display()));
        }
        std::fs::write(&file, &model).map_err(|error| error.to_string())?;
        println!("create {}", file.display());
    } else {
        println!("create {}", file.display());
        print!("{model}");
    }
    Ok(())
}

#[cfg(feature = "schema-import")]
async fn control_connection(
    config: &AppConfig,
    source: &str,
) -> Result<sea_orm::DatabaseConnection, String> {
    let connection = if source == "local" {
        &config.database.primary
    } else {
        config
            .database
            .sources
            .iter()
            .find(|candidate| candidate.name == source)
            .map(|candidate| &candidate.connection)
            .ok_or_else(|| format!("未找到控制库连接 {source}"))?
    };
    ryframe_db::connection::connect_with_sql_logging(
        connection,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await
    .map_err(|error| error.to_string())
}

#[cfg(feature = "schema-import")]
fn snake_case(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err("--table 只能包含字母、数字和下划线".into());
    }
    Ok(value)
}

fn sync_frontend(
    workspace: &std::path::Path,
    settings: &ryframe_config::ToolSettings,
) -> Result<(), String> {
    let frontend = settings.frontend_dir(workspace);
    if !frontend.join("package.json").is_file() {
        return Err(format!(
            "前端目录无效：{}；请配置 workspace.frontend_dir",
            frontend.display()
        ));
    }
    let status = std::process::Command::new("cargo")
        .args(["xtask", "generate", "api", "--write", "--frontend-dir"])
        .arg(frontend)
        .current_dir(workspace)
        .status()
        .map_err(|error| format!("无法启动候选契约同步：{error}；请确认 Cargo 与前端工具链可用"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "候选契约同步失败：{status}；后端生成已完成，修复上述错误后重新执行同步"
        ))
    }
}

mod options;
use options::{Arguments, load_tools, print_plan};
