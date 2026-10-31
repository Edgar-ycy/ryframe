use std::{collections::BTreeMap, fs, path::Path, process::Command};

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use super::super::{CapturedConfiguration, PreparedConfiguration};
use super::{evidence, message, mysql, snapshot, workspace::Fixture};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_external_source_import_migrates_only_the_runtime_control_schema() {
    mysql::run_mysql_test("external_source", |source| async move {
        mysql::execute(&source, "CREATE TABLE table1 (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32) NOT NULL, code VARCHAR(32) NOT NULL, price DECIMAL(20,6) NOT NULL DEFAULT 0.125000, created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), UNIQUE KEY uk_table1_code(code)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci").await?;
        mysql::execute(&source, "INSERT INTO table1(name,code,price,created_at) VALUES('外部结构原行','external-source-marker','90071992547409.123456','2026-10-15 10:20:30.123456')").await?;
        mysql::run_mysql_test("external_control", |runtime| async move {
            let source_identity = ryframe_db::connection::physical_identity(&source).await.map_err(message)?;
            let runtime_identity = ryframe_db::connection::physical_identity(&runtime).await.map_err(message)?;
            assert_ne!(source_identity, runtime_identity);
            assert_ne!(source_identity.database, runtime_identity.database);
            let schema = runtime_identity.database.clone();
            let mut fixture = tokio::task::spawn_blocking(move || Fixture::new(&schema))
                .await.map_err(message)??;
            super::super::verify_physical_binding(&runtime, &fixture.configuration).await?;
            fixture.verify_migration_binding(&runtime).await?;
            let configuration = prepare_source_configuration(&fixture, &source_identity.database)?;
            super::super::verify_physical_binding(&runtime, &configuration).await?;
            verify_selected_source(&source, &configuration).await?;
            fs::write(fixture.directory.join("physical-bindings.json"), serde_json::to_vec_pretty(&serde_json::json!({
                "source": {"server_uuid": source_identity.server_uuid, "database": source_identity.database},
                "runtime": {"server_uuid": runtime_identity.server_uuid, "database": runtime_identity.database},
            })).map_err(message)?).map_err(message)?;
            exercise(&source, &runtime, &fixture, &configuration).await?;
            configuration.verify()?;
            fixture.verify_configuration()?;
            fixture.original.verify()?;
            fixture.completed = true;
            Ok(())
        }).await;
        Ok(())
    }).await;
}

async fn exercise(
    source: &DatabaseConnection,
    runtime: &DatabaseConnection,
    fixture: &Fixture,
    configuration: &PreparedConfiguration,
) -> Result<(), String> {
    let source_before = evidence::TableImage::capture(source).await?;
    let source_catalog = catalog(source).await?;
    assert_eq!(source_catalog.len(), 1);
    assert!(evidence::ledger(source).await?.is_empty());
    source_before.save(&fixture.directory.join("source-before.json"))?;
    save_catalog(
        &fixture.directory.join("source-before-catalog.json"),
        &source_catalog,
    )?;
    let runtime_before = catalog(runtime).await?;
    assert!(
        runtime_before.is_empty(),
        "运行库必须是 fixture 创建的完整空库"
    );
    save_catalog(
        &fixture.directory.join("runtime-before.json"),
        &runtime_before,
    )?;
    let before_preview = snapshot::SourceSnapshot::capture(&fixture.root)?;
    import(fixture, configuration, false, "01-source-preview")?;
    before_preview.verify()?;
    assert_source_unchanged(source, &source_before, &source_catalog).await?;
    assert_eq!(catalog(runtime).await?, runtime_before);
    import(fixture, configuration, true, "02-source-write")?;
    verify_generated_source(fixture, source_catalog.get("table1").ok_or("缺少源表")?)?;
    assert_source_unchanged(source, &source_before, &source_catalog).await?;
    assert_eq!(catalog(runtime).await?, runtime_before);
    migrate_runtime(source, runtime, fixture, &source_before, &source_catalog).await?;
    Ok(())
}

async fn migrate_runtime(
    source: &DatabaseConnection,
    runtime: &DatabaseConnection,
    fixture: &Fixture,
    source_before: &evidence::TableImage,
    source_catalog: &BTreeMap<String, String>,
) -> Result<(), String> {
    let executable = fixture.build("03-external-source-build")?;
    fixture.migrate(&executable, "04-runtime-up", &["control", "up"])?;
    fixture.migrate(&executable, "05-runtime-verify", &["control", "verify"])?;
    fixture.verify_migration_binding(runtime).await?;
    let runtime_after = catalog(runtime).await?;
    assert_eq!(runtime_after.get("table1"), source_catalog.get("table1"));
    assert!(runtime_after.contains_key("ryframe_business_migration"));
    assert!(
        evidence::rows(runtime).await?.is_empty(),
        "结构导入不得复制外部原行"
    );
    let revisions = evidence::versions(&fixture.business())?;
    assert_eq!(revisions.len(), 1);
    evidence::require_origin(runtime, &revisions[0], "executed").await?;
    assert_eq!(evidence::ledger(runtime).await?.len(), 1);
    assert_source_unchanged(source, source_before, source_catalog).await?;
    assert!(!evidence::ledger_exists(source).await?);
    mysql::require_count(source, "SELECT COUNT(*) AS value FROM table1 WHERE id=1 AND name='外部结构原行' AND code='external-source-marker' AND CAST(price AS CHAR)='90071992547409.123456' AND DATE_FORMAT(created_at,'%Y-%m-%d %H:%i:%s.%f')='2026-10-15 10:20:30.123456'", 1).await?;
    save_catalog(
        &fixture.directory.join("runtime-after.json"),
        &runtime_after,
    )?;
    save_catalog(
        &fixture.directory.join("source-after-catalog.json"),
        &catalog(source).await?,
    )?;
    evidence::TableImage::capture(source)
        .await?
        .save(&fixture.directory.join("source-after.json"))?;
    Ok(())
}

fn prepare_source_configuration(
    fixture: &Fixture,
    source_schema: &str,
) -> Result<PreparedConfiguration, String> {
    fixture.verify_configuration()?;
    let declared = fixture.directory.join("external-source-declaration");
    let binding = fixture.directory.join("external-source-binding");
    fs::create_dir(&declared).map_err(message)?;
    fs::create_dir(&binding).map_err(message)?;
    let mut primary = toml::Table::new();
    let mut selected = None;
    for name in ["app.toml", "app.test.toml"] {
        let path = fixture.configuration.directory()?.join(name);
        if !path.exists() {
            continue;
        }
        let bytes = fs::read(path).map_err(message)?;
        let table: toml::Table =
            toml::from_str(std::str::from_utf8(&bytes).map_err(|_| "派生配置不是 UTF-8")?)
                .map_err(|_| "派生配置 TOML 解析失败；不回显配置")?;
        if let Some(connection) = table
            .get("database")
            .and_then(|db| db.get("primary"))
            .and_then(toml::Value::as_table)
        {
            // DbConnection 是平面字段；公共文件先载入，环境文件覆盖同名字段。
            primary.extend(connection.clone());
        }
        if name == "app.test.toml" {
            selected = Some(table);
        } else {
            fs::write(declared.join(name), bytes).map_err(message)?;
        }
    }
    primary.insert("name".into(), "external".into());
    primary.insert("database".into(), source_schema.into());
    let mut selected = selected.ok_or("缺少已捕获的 app.test.toml")?;
    selected
        .get_mut("database")
        .and_then(toml::Value::as_table_mut)
        .ok_or("已捕获 database 必须是表")?
        .insert("sources".into(), toml::Value::Array(vec![primary.into()]));
    fs::write(
        declared.join("app.test.toml"),
        toml::to_string(&selected).map_err(|_| "外部来源配置序列化失败；不回显配置")?,
    )
    .map_err(message)?;
    let runtime = ryframe_config::AppConfig::load(
        fixture.configuration.directory()?,
        ryframe_config::Environment::Test,
    )
    .map_err(|_| "运行配置加载失败；不回显配置")?;
    let prepared = CapturedConfiguration::capture(&declared)?
        .prepare(&binding, &runtime.database.primary.database)?;
    fixture.verify_configuration()?;
    Ok(prepared)
}

async fn verify_selected_source(
    source: &DatabaseConnection,
    configuration: &PreparedConfiguration,
) -> Result<(), String> {
    let app = ryframe_config::AppConfig::load(
        configuration.directory()?,
        ryframe_config::Environment::Test,
    )
    .map_err(|_| "外部来源配置加载失败；不回显配置")?;
    assert_eq!(app.database.sources.len(), 1);
    assert_eq!(app.database.sources[0].name, "external");
    let candidate = ryframe_db::connection::connect(&app.database.sources[0].connection)
        .await
        .map_err(|_| "外部来源配置无法连接 fixture；不回显凭据")?;
    let observed = ryframe_db::connection::physical_identity(&candidate)
        .await
        .map_err(message);
    let closed = candidate.close().await.map_err(message);
    assert_eq!(
        observed?,
        ryframe_db::connection::physical_identity(source)
            .await
            .map_err(message)?
    );
    closed?;
    configuration.verify()
}

fn import(
    fixture: &Fixture,
    configuration: &PreparedConfiguration,
    write: bool,
    stage: &str,
) -> Result<(), String> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ryframe-generate"));
    command
        .current_dir(fixture.business().join("src"))
        .env("APP_ENV", "test")
        .args([
            "import",
            "--package",
            "cli-import-business",
            "--database",
            "control",
            "--connection",
            "sources.external",
            "--tables",
            "table1",
            "--no-sync-frontend",
            "--config-dir",
        ])
        .arg(configuration.directory()?);
    if write {
        command.arg("--write");
    }
    let receipt = fixture.directory.join(stage);
    fs::create_dir(&receipt).map_err(message)?;
    fs::write(receipt.join("command.txt"), format!("{command:?}\n")).map_err(message)?;
    let output = command.output().map_err(message)?;
    fs::write(receipt.join("stdout"), &output.stdout).map_err(message)?;
    fs::write(receipt.join("stderr"), &output.stderr).map_err(message)?;
    fs::write(receipt.join("status.txt"), output.status.to_string()).map_err(message)?;
    configuration.verify()?;
    super::super::require_success(std::process::Output {
        status: output.status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    })
    .map_err(|_| {
        format!(
            "外部来源导入阶段 {stage} 失败；保留 {}，不重放",
            receipt.display()
        )
    })
}

fn verify_generated_source(fixture: &Fixture, source_schema: &str) -> Result<(), String> {
    let model =
        fs::read_to_string(fixture.business().join("src/resources/table1.rs")).map_err(message)?;
    let syntax = syn::parse_file(&model).map_err(message)?;
    let models = syntax
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Struct(model) => Some(model),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(models.len(), 1);
    let names = models[0]
        .fields
        .iter()
        .map(|field| field.ident.as_ref().map(ToString::to_string))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["id", "name", "code", "price", "created_at"].map(|name| Some(name.to_owned()))
    );
    let price = models[0]
        .fields
        .iter()
        .find(|field| field.ident.as_ref().is_some_and(|name| name == "price"))
        .ok_or("导入模型缺少精确小数字段")?;
    assert!(
        matches!(&price.ty, syn::Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Decimal"))
    );
    let history: serde_json::Value = serde_json::from_slice(
        &fs::read(
            fixture
                .business()
                .join("src/generated/migrations/history.json"),
        )
        .map_err(message)?,
    )
    .map_err(message)?;
    let resources = history["resources"].as_array().ok_or("缺少正式迁移资源")?;
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["descriptor"]["database"], "control");
    assert_eq!(resources[0]["descriptor"]["table"], "table1");
    assert_eq!(resources[0]["versions"][0]["canonical"], source_schema);
    Ok(())
}

async fn catalog(database: &DatabaseConnection) -> Result<BTreeMap<String, String>, String> {
    let tables = database.query_all_raw(Statement::from_string(DbBackend::MySql,
        "SELECT table_name AS name FROM information_schema.tables WHERE table_schema=DATABASE() ORDER BY table_name"))
        .await.map_err(message)?;
    let mut result = BTreeMap::new();
    for table in tables {
        let name = table.try_get::<String>("", "name").map_err(message)?;
        let schema = ryframe_db::schema::canonical_table_schema(database, &name)
            .await
            .map_err(message)?;
        result.insert(name, schema);
    }
    Ok(result)
}

async fn assert_source_unchanged(
    source: &DatabaseConnection,
    before: &evidence::TableImage,
    before_catalog: &BTreeMap<String, String>,
) -> Result<(), String> {
    assert_eq!(&evidence::TableImage::capture(source).await?, before);
    assert_eq!(&catalog(source).await?, before_catalog);
    Ok(())
}

fn save_catalog(path: &Path, catalog: &BTreeMap<String, String>) -> Result<(), String> {
    fs::write(path, serde_json::to_vec_pretty(catalog).map_err(message)?).map_err(message)
}
