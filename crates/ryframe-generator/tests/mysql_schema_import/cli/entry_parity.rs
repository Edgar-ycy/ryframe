use super::{
    CapturedConfiguration, GenerationBuilder, GenerationOperation, PreparedConfiguration, frontend,
    import_command, message, mysql, require_success, shared_configuration_directory,
    snapshot::SourceSnapshot, verify_physical_binding,
};
use sea_orm::DatabaseConnection;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

type AssetDelta = Vec<(PathBuf, Option<Vec<u8>>, Option<Vec<u8>>)>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_configuration_and_builder_import_the_same_two_table_assets() {
    if !mysql::integration_enabled() {
        eprintln!("跳过 MySQL 真实协议测试（未验收）：缺少显式 .local-tests/mysql/config 配置目录");
        return;
    }
    assert_eq!(std::env::var("APP_ENV").as_deref(), Ok("test"));
    mysql::run_mysql_test("entryparity", |database| async move {
        let mut frontends = Vec::new();
        for entry in ["cli", "configuration", "builder"] {
            let candidate = frontend::Candidate::from_control()?.map(frontend::Candidate::keep);
            frontends.push((entry, candidate));
        }
        let schema = ryframe_db::connection::physical_identity(&database)
            .await
            .map_err(message)?
            .database;
        let original =
            SourceSnapshot::capture(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))?;
        let directory = retain_scope(&original, &frontends, &schema)?;
        prepare_source(&database).await?;
        let mut first_delta = None;
        let mut first_frontend_delta: Option<AssetDelta> = None;
        let mut configurations: Vec<PreparedConfiguration> = Vec::new();
        for (entry, frontend) in &frontends {
            let entry = *entry;
            let (root, configuration) = prepare_backend(&original, &directory, entry, &schema)?;
            verify_physical_binding(&database, &configuration).await?;
            let sync_frontend = frontend.is_some();
            if let Some(frontend) = frontend {
                frontend.configure(&root)?;
            }
            if entry == "configuration" {
                configure_entry(&root, &configuration, sync_frontend)?;
            }
            let before = SourceSnapshot::capture(&root)?;
            let frontend_before = frontend
                .as_ref()
                .map(|candidate| SourceSnapshot::capture(&candidate.path))
                .transpose()?;
            execute_entry(entry, &root, &configuration, sync_frontend).await?;
            configuration.verify()?;
            let delta = before.changes(&root)?;
            for table in ["table1", "table2"] {
                let resource = format!("crates/cli-import-business/src/resources/{table}.rs");
                assert!(
                    delta
                        .iter()
                        .any(|change| change.relative == Path::new(&resource)
                            && change.before.is_none()
                            && change.after.is_some())
                );
            }
            // 比较完整差集，不排除来源字段，也不正规化候选路径或生成内容。
            let bytes = delta
                .into_iter()
                .map(|change| (change.relative, change.before, change.after))
                .collect::<Vec<_>>();
            if let Some(expected) = &first_delta {
                assert_eq!(&bytes, expected, "{entry} 后端全产物差集不等价");
            } else {
                first_delta = Some(bytes);
            }
            verify_frontend(
                entry,
                frontend,
                frontend_before,
                &root,
                &mut first_frontend_delta,
            )?;
            verify_source(&database).await?;
            configurations.push(configuration);
            for prepared in &configurations {
                prepared.verify()?;
            }
            original.verify()?;
        }
        Ok(())
    })
    .await;
}

fn verify_frontend(
    entry: &str,
    frontend: &Option<frontend::Candidate>,
    before: Option<SourceSnapshot>,
    backend: &Path,
    expected: &mut Option<AssetDelta>,
) -> Result<(), String> {
    match (frontend, before) {
        (Some(frontend), Some(before)) => {
            frontend.verify(backend)?;
            let bytes = before
                .changes(&frontend.path)?
                .into_iter()
                .map(|change| (change.relative, change.before, change.after))
                .collect::<AssetDelta>();
            assert!(!bytes.is_empty(), "{entry} 未生成任何前端产物");
            if let Some(first) = expected.as_ref() {
                assert_eq!(&bytes, first, "{entry} 前端全产物差集不等价");
            } else {
                *expected = Some(bytes);
            }
            Ok(())
        }
        (None, None) => Ok(()),
        _ => Err("前端候选与导入前像不一致".into()),
    }
}

fn prepare_backend(
    original: &SourceSnapshot,
    directory: &Path,
    entry: &str,
    schema: &str,
) -> Result<(PathBuf, PreparedConfiguration), String> {
    let root = directory.join(entry);
    original.copy_to(&root)?;
    frontend::configure_target(&root, &original.root)?;
    GenerationBuilder::new(&root)
        .operation(GenerationOperation::NewBusiness("cli-import".into()))
        .write(true)
        .plan()?
        .execute()?;
    let configuration = CapturedConfiguration::capture(&shared_configuration_directory()?)?
        .prepare(&root, schema)?;
    Ok((root, configuration))
}

fn retain_scope(
    original: &SourceSnapshot,
    frontends: &[(&str, Option<frontend::Candidate>)],
    schema: &str,
) -> Result<PathBuf, String> {
    let cache = original.root.join(".local-tests/cli-import");
    fs::create_dir_all(&cache).map_err(message)?;
    let directory = tempfile::tempdir_in(cache).map_err(message)?.keep();
    let receipt = directory.join("scope.json");
    println!(
        "→ 三入口验收候选成功或失败均保留，归属收据：{}",
        receipt.display()
    );
    let scope = serde_json::json!({
        "case": "cli::entry_parity::cli_configuration_and_builder_import_the_same_two_table_assets",
        "pid": std::process::id(),
        "schema": schema,
        "backend_scope": directory,
        "backend_candidates": (["cli", "configuration", "builder"].map(|name| directory.join(name))),
        "frontends": (frontends.iter().map(|(entry, frontend)| serde_json::json!({
            "entry": entry, "path": frontend.as_ref().map(|candidate| &candidate.path),
        })).collect::<Vec<_>>()),
        "frontend_enabled": (frontends.iter().all(|(_, frontend)| frontend.is_some())),
        "original_source_root": original.root,
    });
    fs::write(
        &receipt,
        serde_json::to_vec_pretty(&scope).map_err(message)?,
    )
    .map_err(message)?;
    let first = frontends.first().ok_or("缺少前端候选")?;
    if !frontends
        .iter()
        .all(|(_, frontend)| frontend.is_some() == first.1.is_some())
    {
        return Err("三入口前端候选不允许部分启用".into());
    }
    let before = first
        .1
        .as_ref()
        .map(|candidate| SourceSnapshot::capture(&candidate.path))
        .transpose()?;
    for (_, frontend) in frontends {
        if let (Some(before), Some(frontend)) = (&before, frontend) {
            assert!(
                before.changes(&frontend.path)?.is_empty(),
                "前端候选原始前像不一致"
            );
        }
    }
    if before.is_none() {
        println!("→ 无前端资源控制：本次真实执行三入口后端等价验收，不包含前端验收");
    }
    Ok(directory)
}

async fn prepare_source(database: &DatabaseConnection) -> Result<(), String> {
    mysql::execute(database, "CREATE TABLE table1 (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32) NOT NULL, price DECIMAL(20,6) NOT NULL DEFAULT 0.125000, created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), UNIQUE KEY uk_table1_name(name)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci").await?;
    mysql::execute(database, "CREATE TABLE table2 (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, reference_id BIGINT NULL, name VARCHAR(32) NOT NULL, created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6), KEY idx_table2_reference(reference_id), CONSTRAINT fk_table2_reference FOREIGN KEY(reference_id) REFERENCES table1(id) ON UPDATE RESTRICT ON DELETE SET NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci").await?;
    mysql::execute(
        database,
        "INSERT INTO table1(name,price) VALUES('existing','90071992547409.123456')",
    )
    .await?;
    mysql::execute(
        database,
        "INSERT INTO table2(reference_id,name) VALUES(1,'linked')",
    )
    .await
}

async fn verify_source(database: &DatabaseConnection) -> Result<(), String> {
    for (query, count) in [
        ("SELECT COUNT(*) AS value FROM table1", 1),
        (
            "SELECT COUNT(*) AS value FROM table1 WHERE name='existing' AND CAST(price AS CHAR)='90071992547409.123456'",
            1,
        ),
        ("SELECT COUNT(*) AS value FROM table2", 1),
        (
            "SELECT COUNT(*) AS value FROM table2 WHERE reference_id=1 AND name='linked'",
            1,
        ),
        (
            "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE()",
            2,
        ),
    ] {
        mysql::require_count(database, query, count).await?;
    }
    Ok(())
}

fn configure_entry(
    root: &Path,
    configuration: &PreparedConfiguration,
    sync_frontend: bool,
) -> Result<(), String> {
    let path = root.join(".local-tests/tools.toml");
    let mut tools: toml::Table =
        toml::from_str(&fs::read_to_string(&path).map_err(message)?).map_err(message)?;
    let generator = tools
        .get_mut("generator")
        .and_then(toml::Value::as_table_mut)
        .ok_or("现有工具配置缺少 generator 表")?;
    for (key, value) in [
        ("package", "cli-import-business"),
        ("source", "primary"),
        ("storage", "control"),
    ] {
        generator.insert(key.into(), value.into());
    }
    generator.insert(
        "tables".into(),
        toml::Value::Array(
            ["table1", "table2"]
                .into_iter()
                .map(|table| table.into())
                .collect(),
        ),
    );
    generator.insert("sync_frontend".into(), sync_frontend.into());
    generator.insert(
        "config_dir".into(),
        configuration
            .directory()?
            .to_str()
            .ok_or("私有配置目录不是 UTF-8")?
            .into(),
    );
    fs::write(path, toml::to_string(&tools).map_err(message)?).map_err(message)
}

async fn execute_entry(
    entry: &str,
    root: &Path,
    configuration: &PreparedConfiguration,
    sync_frontend: bool,
) -> Result<(), String> {
    match entry {
        "cli" => {
            let mut command = import_command(root, configuration, "table1,table2")?;
            require_success(
                command
                    .args([
                        if sync_frontend {
                            "--sync-frontend"
                        } else {
                            "--no-sync-frontend"
                        },
                        "--write",
                    ])
                    .output()
                    .map_err(message)?,
            )
        }
        "configuration" => require_success(
            Command::new(env!("CARGO_BIN_EXE_ryframe-generate"))
                .args(["run", "--write"])
                .current_dir(root.join("crates/cli-import-business/src"))
                .env("APP_ENV", "test")
                .output()
                .map_err(message)?,
        ),
        "builder" => {
            let root = root.to_owned();
            let directory = configuration.directory()?.to_owned();
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                GenerationBuilder::new(root)
                    .operation(GenerationOperation::Import)
                    .configuration_directory(directory)
                    .include_tables(["table1", "table2"])
                    .source("primary")
                    .database("control")
                    .package("cli-import-business")
                    .sync_frontend(sync_frontend)
                    .write(true)
                    .plan()?
                    .execute()?;
                Ok(())
            })
            .await
            .map_err(message)?
        }
        _ => Err("未知三入口验收分支".into()),
    }
}
