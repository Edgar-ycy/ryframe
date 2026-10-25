use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use ryframe_generator::command::{GenerationBuilder, GenerationOperation};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

use super::mysql;

#[path = "cli/configuration.rs"]
mod configuration;
use configuration::{CapturedConfiguration, PreparedConfiguration};

#[path = "cli/frontend.rs"]
mod frontend;
#[path = "../../src/command/staging/snapshot.rs"]
mod snapshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_cli_imports_two_tables_from_business_subdirectory_without_database_writes() {
    mysql::run_mysql_test("cliimport", |database| async move {
        let frontend = frontend::Candidate::from_control()?;
        let captured = CapturedConfiguration::capture(&shared_configuration_directory()?)?;
        mysql::execute(&database, "CREATE TABLE table1 (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32) NOT NULL, price DECIMAL(20,6) NOT NULL DEFAULT 0.125000, created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), UNIQUE KEY uk_table1_name(name)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci").await?;
        mysql::execute(&database, "CREATE TABLE table2 (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, reference_id BIGINT NULL, name VARCHAR(32) NOT NULL, created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6), KEY idx_table2_reference(reference_id), CONSTRAINT fk_table2_reference FOREIGN KEY(reference_id) REFERENCES table1(id) ON UPDATE RESTRICT ON DELETE SET NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci").await?;
        mysql::execute(&database, "CREATE TABLE unsupported_table (id BIGINT NOT NULL PRIMARY KEY, payload BIT(8) NOT NULL) ENGINE=InnoDB").await?;
        mysql::execute(&database, "INSERT INTO table1(name,price) VALUES('existing','90071992547409.123456')").await?;
        let schema = database.query_one_raw(Statement::from_string(DbBackend::MySql, "SELECT DATABASE() AS name"))
            .await.map_err(message)?.ok_or("缺少测试库名称")?.try_get::<String>("", "name").map_err(message)?;
        let original = snapshot::SourceSnapshot::capture(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))?;
        assert!(original.input_paths().any(|path| path == Path::new("Cargo.toml")));
        let cache = original.root.join(".local-tests/cli-import");
        fs::create_dir_all(&cache).map_err(message)?;
        let directory = tempfile::tempdir_in(cache).map_err(message)?;
        let root = directory.path().join("backend");
        original.copy_to(&root)?;
        if let Some(frontend) = &frontend {
            frontend.configure_target(&root)?;
        }
        GenerationBuilder::new(&root).operation(GenerationOperation::NewBusiness("cli-import".into())).write(true).plan()?.execute()?;
        let configuration = captured.prepare(&root, &schema)?;
        verify_physical_binding(&database, &configuration).await?;
        let before = snapshot::SourceSnapshot::capture(&root)?;
        let output = import(&root, &configuration, "table1,unsupported_table", true)?;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("BIT(8)") || String::from_utf8_lossy(&output.stderr).contains("bit(8)"));
        before.verify()?;
        require_success(import(&root, &configuration, "table1,table2", false)?)?;
        before.verify()?;
        require_success(import(&root, &configuration, "table1,table2", true)?)?;
        let business = root.join("crates/cli-import-business");
        for resource in ["table1", "table2"] {
            assert!(business.join(format!("src/resources/{resource}.rs")).is_file());
            assert!(business.join(format!(".ryframe/imports/{resource}.json")).is_file());
            for kind in ["entities", "services", "repositories", "handlers", "dto"] {
                assert!(business.join(format!("src/generated/{kind}/{resource}.rs")).is_file());
            }
        }
        let installed = snapshot::SourceSnapshot::capture(&root)?;
        verify_generated_schema(&business).await?;
        let delta = before.changes(&root)?;
        assert!(delta.iter().any(|change| change.relative == Path::new("crates/cli-import-business/src/resources/table1.rs") && change.before.is_none() && change.after.is_some()));
        require_success(import(&root, &configuration, "table1,table2", true)?)?;
        installed.verify()?;
        if let Some(frontend) = &frontend {
            frontend.configure(&root)?;
            let mut command = import_command(&root, &configuration, "table1,table2")?;
            command.args(["--sync-frontend", "--write"]);
            let output = command.output().map_err(message)?;
            configuration.verify()?;
            require_success(output)?;
            frontend.verify(&root)?;
            assert!(installed.changes(&root)?.iter().all(|change| change.relative == Path::new("openapi/openapi.json")));
        }
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM table1 WHERE name='existing' AND CAST(price AS CHAR)='90071992547409.123456'", 1).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE()", 3).await?;
        configuration.verify()?;
        original.verify()?;
        Ok(())
    }).await;
}

fn import(
    root: &Path,
    configuration: &PreparedConfiguration,
    tables: &str,
    write: bool,
) -> Result<Output, String> {
    let mut command = import_command(root, configuration, tables)?;
    command.arg("--no-sync-frontend");
    if write {
        command.arg("--write");
    }
    let output = command.output().map_err(message)?;
    configuration.verify()?;
    Ok(output)
}

async fn verify_generated_schema(business: &Path) -> Result<(), String> {
    let history: serde_json::Value = serde_json::from_slice(
        &fs::read(business.join("src/generated/migrations/history.json")).map_err(message)?,
    )
    .map_err(message)?;
    let business = business.to_owned();
    mysql::run_mysql_test("cli-import-fresh", |database| async move {
        for resource in history["resources"].as_array().ok_or("缺少迁移资源")? {
            let initial = &resource["versions"][0];
            let id = initial["id"].as_str().ok_or("缺少初始迁移 ID")?;
            super::business_evolution::execute_migration(
                &database,
                &business.join(format!("migrations/{id}.rs")),
            )
            .await?;
            let table = resource["descriptor"]["table"]
                .as_str()
                .ok_or("缺少迁移表名")?;
            let actual = ryframe_db::schema::canonical_table_schema(&database, table)
                .await
                .map_err(mysql::db_error)?;
            assert_eq!(actual, initial["canonical"].as_str().ok_or("缺少初始结构")?);
        }
        Ok(())
    })
    .await;
    Ok(())
}

fn import_command(
    root: &Path,
    configuration: &PreparedConfiguration,
    tables: &str,
) -> Result<Command, String> {
    let directory = configuration.directory()?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_ryframe-generate"));
    command
        .args([
            "import",
            "--package",
            "cli-import-business",
            "--database",
            "control",
            "--connection",
            "primary",
            "--tables",
            tables,
        ])
        .arg("--config-dir")
        .arg(directory)
        .current_dir(root.join("crates/cli-import-business/src"))
        .env("APP_ENV", "test");
    Ok(command)
}

fn shared_configuration_directory() -> Result<std::path::PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|root| root.join("architecture/crate-boundaries.toml").is_file())
        .map(|root| root.join(".local-tests/mysql/config"))
        .ok_or_else(|| "无法定位共享 MySQL 配置的后端根目录".into())
}

async fn verify_physical_binding(
    database: &sea_orm::DatabaseConnection,
    configuration: &PreparedConfiguration,
) -> Result<(), String> {
    let expected = ryframe_db::connection::physical_identity(database)
        .await
        .map_err(|_| "无法核对外层 MySQL fixture 物理身份".to_owned())?;
    let configuration_directory = configuration.directory()?;
    let app = ryframe_config::AppConfig::load(
        configuration_directory,
        ryframe_config::Environment::Test,
    )
    .map_err(|error| format!("阶段=校验捕获的私有配置；路径={}；原因={}；处理=按文件与字段提示修正共享配置并核对fixture绑定；已创建目标保留，不自动重放", configuration_directory.display(), error.message()))?;
    let candidate = ryframe_db::connection::connect(&app.database.primary)
        .await
        .map_err(|_| "捕获的 MySQL 配置无法连接 fixture schema；未执行导入".to_owned())?;
    let observed = ryframe_db::connection::physical_identity(&candidate)
        .await
        .map_err(|_| "无法核对捕获配置的 MySQL 物理身份".to_owned());
    let closed = candidate
        .close()
        .await
        .map_err(|_| "关闭 MySQL 物理身份核验连接失败".to_owned());
    let observed = observed?;
    closed?;
    configuration.verify()?;
    if observed != expected {
        return Err("捕获配置与外层 MySQL fixture 的物理实例或 schema 不一致；未执行导入".into());
    }
    Ok(())
}

fn require_success(output: Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "正式导入命令失败（{}）：{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
