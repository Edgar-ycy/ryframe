use std::{fs, path::Path};

use sea_orm::DatabaseConnection;

use super::{GenerationBuilder, GenerationOperation, message, mysql, snapshot};

#[path = "adoption_flow/commands.rs"]
mod commands;
#[path = "adoption_flow/evidence.rs"]
mod evidence;
#[path = "adoption_flow/workspace.rs"]
mod workspace;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_import_adopt_and_incremental_control_migration_preserve_existing_rows() {
    mysql::run_mysql_test("cliadoptflow", |database| async move {
        let schema = ryframe_db::connection::physical_identity(&database)
            .await
            .map_err(message)?
            .database;
        let mut fixture = tokio::task::spawn_blocking(move || workspace::Fixture::new(&schema))
            .await
            .map_err(message)??;
        super::verify_physical_binding(&database, &fixture.configuration).await?;
        fixture.verify_migration_binding(&database).await?;
        let empty = fixture.build("01-empty")?;
        fixture.migrate(&empty, "02-initialize", &["control", "up"])?;
        fixture.migrate(&empty, "03-empty-verify", &["control", "verify"])?;
        exercise(&database, &fixture).await?;
        fixture.configuration.verify()?;
        fixture.original.verify()?;
        fixture.completed = true;
        Ok(())
    })
    .await;
}

async fn exercise(
    database: &DatabaseConnection,
    fixture: &workspace::Fixture,
) -> Result<(), String> {
    mysql::execute(database, "CREATE TABLE table1 (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32) NOT NULL, code VARCHAR(32) NOT NULL, price DECIMAL(20,6) NOT NULL DEFAULT 0.125000, created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), UNIQUE KEY uk_table1_code(code)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci").await?;
    mysql::execute(database, "INSERT INTO table1(name,code,price,created_at) VALUES('保留原行','original-code','90071992547409.123456','2026-10-15 10:20:30.123456')").await?;
    mysql::require_count(database, "SELECT COUNT(*) AS value FROM table1 WHERE id=1 AND name='保留原行' AND code='original-code' AND CAST(price AS CHAR)='90071992547409.123456' AND DATE_FORMAT(created_at,'%Y-%m-%d %H:%i:%s.%f')='2026-10-15 10:20:30.123456'", 1).await?;
    let original = evidence::TableImage::capture(database).await?;
    original.save(&fixture.directory.join("before-import.json"))?;
    fixture.import()?;
    let imported = fixture.build("05-imported")?;
    let initial = evidence::versions(&fixture.business())?;
    if initial.len() != 1 {
        return Err("正式导入必须产生唯一初始迁移".into());
    }
    let initial_path = fixture
        .business()
        .join(format!("migrations/{}.rs", initial[0]));
    let initial_bytes = fs::read(&initial_path).map_err(message)?;
    let adopted = adopt(database, fixture, &imported, &original, &initial[0]).await?;
    evolve(&fixture.root).await?;
    assert_eq!(fs::read(initial_path).map_err(message)?, initial_bytes);
    let revisions = evidence::versions(&fixture.business())?;
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0], initial[0]);
    let evolved = fixture.build("09-evolved")?;
    fixture.migrate(&evolved, "10-incremental-up", &["control", "up"])?;
    fixture.migrate(&evolved, "11-current-verify", &["control", "verify"])?;
    assert_eq!(evidence::rows(database).await?, original.rows);
    mysql::require_count(
        database,
        "SELECT COUNT(*) AS value FROM table1 WHERE memo IS NULL",
        1,
    )
    .await?;
    evidence::require_origin(database, &initial[0], "adopted").await?;
    evidence::require_origin(database, &revisions[1], "executed").await?;
    let final_ledger = evidence::ledger(database).await?;
    assert_eq!(final_ledger.len(), 2);
    for row in &adopted {
        assert!(final_ledger.contains(row));
    }
    let final_table = evidence::TableImage::capture(database).await?;
    fixture.migrate(&evolved, "12-repeat-up", &["control", "up"])?;
    fixture.migrate(&evolved, "13-repeat-verify", &["control", "verify"])?;
    assert_eq!(evidence::TableImage::capture(database).await?, final_table);
    assert_eq!(evidence::ledger(database).await?, final_ledger);
    Ok(())
}

async fn adopt(
    database: &DatabaseConnection,
    fixture: &workspace::Fixture,
    imported: &Path,
    original: &evidence::TableImage,
    initial: &str,
) -> Result<Vec<String>, String> {
    let before_exists = evidence::ledger_exists(database).await?;
    let before = evidence::ledger(database).await?;
    let plan = fixture.root.join(".local-tests/adoption-plan.json");
    let plan_text = plan.to_str().ok_or("接管计划路径不是 UTF-8")?;
    fixture.migrate(
        imported,
        "06-preview",
        &[
            "control",
            "adopt",
            "--module",
            "cli-import",
            "--output",
            plan_text,
            "--write",
        ],
    )?;
    assert_eq!(evidence::ledger(database).await?, before);
    assert_eq!(evidence::ledger_exists(database).await?, before_exists);
    assert_eq!(&evidence::TableImage::capture(database).await?, original);
    let plan_bytes = fs::read(&plan).map_err(message)?;
    fixture.migrate(
        imported,
        "07-apply",
        &["control", "adopt", "--apply", plan_text, "--write"],
    )?;
    let inspected = fixture.migrate(
        imported,
        "08-inspect",
        &["control", "adopt", "--inspect", plan_text],
    )?;
    let state: serde_json::Value = serde_json::from_slice(&inspected.stdout).map_err(message)?;
    assert_eq!(state, serde_json::json!("applied"));
    assert_eq!(fs::read(&plan).map_err(message)?, plan_bytes);
    assert_eq!(&evidence::TableImage::capture(database).await?, original);
    let adopted = evidence::ledger(database).await?;
    assert_eq!(adopted.len(), 1);
    evidence::require_origin(database, initial, "adopted").await?;
    fs::write(
        fixture.directory.join("adopted-ledger.json"),
        serde_json::to_vec_pretty(&adopted).map_err(message)?,
    )
    .map_err(message)?;
    Ok(adopted)
}

async fn evolve(root: &Path) -> Result<(), String> {
    let root = root.to_owned();
    tokio::task::spawn_blocking(move || {
        let model = root.join("crates/cli-import-business/src/resources/table1.rs");
        let source = fs::read_to_string(&model).map_err(message)?;
        let marker = "pub code: String,";
        if source.matches(marker).count() != 1 {
            return Err("导入模型没有唯一公开 code 字段；保留现场，不猜测模型插入点".into());
        }
        fs::write(
            model,
            source.replacen(
                marker,
                "pub code: String,\n    pub memo: Option<String>,",
                1,
            ),
        )
        .map_err(message)?;
        GenerationBuilder::new(&root)
            .operation(GenerationOperation::Resource)
            .package("cli-import-business")
            .write(true)
            .sync_frontend(false)
            .plan()?
            .execute()?;
        Ok(())
    })
    .await
    .map_err(message)?
}
