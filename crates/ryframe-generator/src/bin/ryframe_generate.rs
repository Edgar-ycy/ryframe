use ryframe_generator::business::{
    BusinessBootstrapOptions, BusinessGenerateOptions, bootstrap_business_package,
    generate_business_package, locate_business_package, workspace_root,
};

#[cfg(feature = "schema-import")]
use ryframe_config::{AppConfig, Environment};
#[cfg(feature = "schema-import")]
use ryframe_generator::import::{inspect_existing_table, rust_model_source};
#[cfg(feature = "schema-import")]
use ryframe_tenant_db::TenantDatabaseTargetRegistry;

const USAGE: &str = "用法：\n  ryframe-generate new-business <模块名> [--dry-run]\n  ryframe-generate resource --package <crate> [--model <Model>] [--write] [--sync-frontend]\n  ryframe-generate import --connection <name> --database <control|tenant> --package <crate> --table <table> [--write]";

#[cfg(feature = "schema-import")]
#[tokio::main]
async fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()).await {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

#[cfg(not(feature = "schema-import"))]
fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

#[cfg(feature = "schema-import")]
async fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(USAGE.into());
    };
    match command {
        "new-business" => new_business(&args[1..]),
        "resource" => resource(&args[1..]),
        "import" => import(&args[1..]).await,
        _ => Err(USAGE.into()),
    }
}

#[cfg(not(feature = "schema-import"))]
fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("new-business") => new_business(&args[1..]),
        Some("resource") => resource(&args[1..]),
        Some("import") => Err("数据库导入需要使用 --features schema-import".into()),
        _ => Err(USAGE.into()),
    }
}

fn new_business(args: &[String]) -> Result<(), String> {
    let [module, rest @ ..] = args else {
        return Err(USAGE.into());
    };
    let dry_run = match rest {
        [] => false,
        [flag] if flag == "--dry-run" => true,
        _ => return Err(USAGE.into()),
    };
    let current_dir =
        std::env::current_dir().map_err(|error| format!("无法读取当前目录：{error}"))?;
    let workspace_root = workspace_root(&current_dir).map_err(|error| error.to_string())?;
    let report = bootstrap_business_package(BusinessBootstrapOptions {
        workspace_root: &workspace_root,
        module,
        write: !dry_run,
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
        if dry_run { "预览" } else { "创建完成" },
        report.created.len(),
        report.updated.len(),
    );
    Ok(())
}

fn resource(args: &[String]) -> Result<(), String> {
    let parsed = Arguments::parse(args)?;
    let package = parsed.required("--package")?;
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
    if parsed.flag("--sync-frontend") {
        sync_frontend(&workspace_root, parsed.flag("--write"))?;
    }
    Ok(())
}

#[cfg(feature = "schema-import")]
async fn import(args: &[String]) -> Result<(), String> {
    let parsed = Arguments::parse_import(args)?;
    let package = parsed.required("--package")?;
    let scope = parsed.required("--database")?;
    let current = std::env::current_dir().map_err(|error| format!("无法读取当前目录：{error}"))?;
    let target = locate_business_package(&current, package).map_err(|error| error.to_string())?;
    let config =
        AppConfig::load_from_env(Environment::from_env().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let table = parsed.required("--table")?;
    let source = parsed.required("--connection")?;
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

fn sync_frontend(workspace: &std::path::Path, write: bool) -> Result<(), String> {
    if !write {
        return Err("--sync-frontend 必须与 --write 一起使用".into());
    }
    let frontend = std::env::var_os("RYFRAME_GENERATOR_FRONTEND_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| workspace.parent().map(|parent| parent.join("ryframe-vue3")))
        .filter(|path| path.join("package.json").is_file())
        .ok_or_else(|| {
            "无法定位前端工作区：请设置 RYFRAME_GENERATOR_FRONTEND_DIR 或在后端同级放置 ryframe-vue3"
                .to_owned()
        })?;
    let corepack = if cfg!(windows) {
        "corepack.cmd"
    } else {
        "corepack"
    };
    let status = std::process::Command::new(corepack)
        .args(["pnpm", "generate", "--write"])
        .current_dir(frontend)
        .status()
        .map_err(|error| format!("无法启动前端正式生成入口：{error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("前端生成失败：{status}"))
    }
}

struct Arguments {
    values: Vec<(String, String)>,
    flags: Vec<String>,
}

impl Arguments {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut values = Vec::new();
        let mut flags = Vec::new();
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--write" | "--sync-frontend" => {
                    flags.push(args[index].clone());
                    index += 1;
                }
                "--package" | "--model" => {
                    let value = args.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
                    values.push((args[index].clone(), value.clone()));
                    index += 2;
                }
                _ => return Err(USAGE.into()),
            }
        }
        Ok(Self { values, flags })
    }

    #[cfg(feature = "schema-import")]
    fn parse_import(args: &[String]) -> Result<Self, String> {
        let parsed = Self::parse_with_values(
            args,
            &["--connection", "--database", "--package", "--table"],
        )?;
        if parsed.flag("--sync-frontend") || parsed.value("--model").is_some() {
            return Err(USAGE.into());
        }
        Ok(parsed)
    }

    #[cfg(feature = "schema-import")]
    fn parse_with_values(args: &[String], names: &[&str]) -> Result<Self, String> {
        let mut values = Vec::new();
        let mut flags = Vec::new();
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--write" | "--sync-frontend" => {
                    flags.push(args[index].clone());
                    index += 1;
                }
                value if names.contains(&value) || value == "--model" => {
                    let next = args.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
                    values.push((args[index].clone(), next.clone()));
                    index += 2;
                }
                _ => return Err(USAGE.into()),
            }
        }
        Ok(Self { values, flags })
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn required(&self, name: &str) -> Result<&str, String> {
        self.value(name)
            .ok_or_else(|| format!("缺少 {name}\n{USAGE}"))
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|value| value == name)
    }
}
