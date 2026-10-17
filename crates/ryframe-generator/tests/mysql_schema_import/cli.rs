use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use ryframe_generator::command::{GenerationBuilder, GenerationOperation};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

use super::mysql;

#[path = "cli/frontend.rs"]
mod frontend;
#[path = "../../src/command/staging/snapshot.rs"]
mod snapshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_cli_imports_two_tables_from_business_subdirectory_without_database_writes() {
    mysql::run_mysql_test("cliimport", |database| async move {
        mysql::execute(&database, "CREATE TABLE table1 (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32) NOT NULL, price DECIMAL(20,6) NOT NULL DEFAULT 0.125000, UNIQUE KEY uk_table1_name(name)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci").await?;
        mysql::execute(&database, "CREATE TABLE table2 (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, reference_id BIGINT NULL, name VARCHAR(32) NOT NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci").await?;
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
        GenerationBuilder::new(&root).operation(GenerationOperation::NewBusiness("cli-import".into())).write(true).plan()?.execute()?;
        let before = snapshot::SourceSnapshot::capture(&root)?;
        let output = import(&root, &schema, "table1,unsupported_table", true)?;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("BIT(8)") || String::from_utf8_lossy(&output.stderr).contains("bit(8)"));
        before.verify()?;
        require_success(import(&root, &schema, "table1,table2", false)?)?;
        before.verify()?;
        require_success(import(&root, &schema, "table1,table2", true)?)?;
        let business = root.join("crates/cli-import-business");
        for resource in ["table1", "table2"] {
            assert!(business.join(format!("src/resources/{resource}.rs")).is_file());
            assert!(business.join(format!(".ryframe/imports/{resource}.json")).is_file());
            for kind in ["entities", "services", "repositories", "handlers", "dto"] {
                assert!(business.join(format!("src/generated/{kind}/{resource}.rs")).is_file());
            }
        }
        let installed = snapshot::SourceSnapshot::capture(&root)?;
        let delta = before.changes(&root)?;
        assert!(delta.iter().any(|change| change.relative == Path::new("crates/cli-import-business/src/resources/table1.rs") && change.before.is_none() && change.after.is_some()));
        require_success(import(&root, &schema, "table1,table2", true)?)?;
        installed.verify()?;
        if let Some(frontend) = frontend::Candidate::from_environment()? {
            frontend.configure(&root)?;
            let mut command = import_command(&root, &schema, "table1,table2");
            command.args(["--sync-frontend", "--write"]);
            require_success(command.output().map_err(message)?)?;
            frontend.verify(&root)?;
            assert!(installed.changes(&root)?.iter().all(|change| change.relative == Path::new("openapi/openapi.json")));
        }
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM table1 WHERE name='existing' AND CAST(price AS CHAR)='90071992547409.123456'", 1).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE()", 3).await?;
        original.verify()?;
        Ok(())
    }).await;
}

fn import(root: &Path, schema: &str, tables: &str, write: bool) -> Result<Output, String> {
    let mut command = import_command(root, schema, tables);
    command.arg("--no-sync-frontend");
    if write {
        command.arg("--write");
    }
    command.output().map_err(message)
}

fn import_command(root: &Path, schema: &str, tables: &str) -> Command {
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
        .current_dir(root.join("crates/cli-import-business/src"))
        .env_remove("APP_CONFIG_DIR")
        .env("APP_ENV", "dev")
        .env("APP_DATABASE_NAME", schema)
        .env("APP_DATABASE_TLS_MODE", "required");
    command.env(
        "CARGO_TARGET_DIR",
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/mysql-schema-cli"),
    );
    for (suffix, default) in [
        ("HOST", "127.0.0.1"),
        ("PORT", "3306"),
        ("USERNAME", "root"),
        ("PASSWORD", ""),
    ] {
        command.env(
            format!("APP_DATABASE_{suffix}"),
            std::env::var(format!("RYFRAME_MYSQL_{suffix}")).unwrap_or_else(|_| default.into()),
        );
    }
    command
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
