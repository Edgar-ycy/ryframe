use std::{fs, path::Path, process::Command};

use ryframe_generator::business::{BusinessGenerateOptions, generate_business_package};
use sea_orm::{ConnectionTrait, DatabaseConnection};

use super::mysql;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generated_incremental_sql_preserves_rows_and_matches_current_mysql_schema() {
    mysql::run_mysql_test("generatedevolution", |database| async move {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests");
        fs::create_dir_all(&root).map_err(message)?;
        let workspace = tempfile::Builder::new().prefix("business-evolution-").tempdir_in(root).map_err(message)?;
        prepare(workspace.path(), MODEL)?;
        let root = workspace.path().join("evolution-business");
        let initial = generate(workspace.path())?;
        let initial = initial.created.iter().find(|path| path.starts_with("migrations/")).ok_or("缺少初始迁移")?;
        let initial_bytes = fs::read(root.join(initial)).map_err(message)?;
        execute_migration(&database, &root.join(initial)).await?;
        mysql::execute(&database, "INSERT INTO biz_evolution(code) VALUES('preserve-existing-row')").await?;
        let model = root.join("src/resources/mod.rs");
        fs::write(&model, MODEL.replace("    pub code: String,", "    pub code: String,\n    pub memo: Option<String>,\n    #[resource(default = \"0\")]\n    pub amount: i32,\n    #[resource(sql_type = \"DECIMAL(20,6)\")]\n    pub price: Option<ryframe_sdk::rust_decimal::Decimal>,")).map_err(message)?;
        let first = generate(workspace.path())?;
        let first = first.created.iter().find(|path| path.starts_with("migrations/")).ok_or("缺少第一次追加迁移")?;
        execute_migration(&database, &root.join(first)).await?;
        verify_current(&database, &root).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_evolution WHERE id=1 AND code='preserve-existing-row' AND memo IS NULL AND amount=0", 1).await?;
        mysql::execute(&database, "UPDATE biz_evolution SET memo='keep memo',amount=2147483647,price='90071992547409.123456' WHERE id=1").await?;
        fs::write(&model, fs::read_to_string(&model).map_err(message)?.replace("pub amount: i32", "pub amount: i64").replace("DECIMAL(20,6)", "DECIMAL(22,8)")).map_err(message)?;
        let second = generate(workspace.path())?;
        let second = second.created.iter().find(|path| path.starts_with("migrations/")).ok_or("缺少第二次追加迁移")?;
        execute_migration(&database, &root.join(second)).await?;
        verify_current(&database, &root).await?;
        mysql::execute(&database, "UPDATE biz_evolution SET amount=2147483648 WHERE id=1").await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_evolution WHERE id=1 AND code='preserve-existing-row' AND memo='keep memo' AND amount=2147483648 AND CAST(price AS CHAR)='90071992547409.12345600'", 1).await?;
        assert_eq!(fs::read(root.join(initial)).map_err(message)?, initial_bytes);
        let repeat = generate(workspace.path())?;
        assert!(repeat.created.is_empty() && repeat.updated.is_empty() && repeat.removed.is_empty());
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn imported_physical_types_match_existing_table_and_generated_ddl() {
    mysql::run_mysql_test("importedtypes", |database| async move {
        mysql::execute(&database, "CREATE TABLE biz_evolution (id BIGINT NOT NULL PRIMARY KEY, code VARCHAR(32) COLLATE utf8mb4_bin NOT NULL DEFAULT _utf8mb4 X'66726573685c70617468', memo TEXT NULL, amount INT NOT NULL DEFAULT 7, occurred_at DATETIME(3) NULL, whole_second DATETIME NULL, payload JSON NULL, tiny_count TINYINT NULL, small_count SMALLINT NULL, byte_count TINYINT UNSIGNED NULL, short_count SMALLINT UNSIGNED NULL, count INT UNSIGNED NULL, large_count BIGINT UNSIGNED NULL DEFAULT 18446744073709551615, created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), updated_at DATETIME(6) NULL ON UPDATE CURRENT_TIMESTAMP(6), price DECIMAL(20,6) NOT NULL DEFAULT 0.125000, exact_amount DECIMAL(28,6) NULL, UNIQUE KEY uk_external_code(code), KEY idx_external_amount(amount,code)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci").await?;
        mysql::execute(&database, "INSERT INTO biz_evolution(id,code,amount) VALUES(41,'keep-source',7)").await?;
        let table = ryframe_generator::import::inspect_existing_table(&database, "biz_evolution").await.map_err(message)?;
        let model = ryframe_generator::import::rust_model_source(&table, "control")?;
        assert!(model.contains("sql_type = \"varchar(32)\""));
        assert!(model.contains("sql_type = \"datetime(3)\""));
        assert!(model.contains("collation = \"utf8mb4_0900_ai_ci\""));
        assert!(model.contains("collation = \"utf8mb4_bin\""));
        let model = model.replace("name = \"biz_evolution\"", "name = \"evolution\"")
            .replace("sql_type = \"datetime(3)\"", "read_only, sql_type = \"datetime(3)\"")
            .replace("sql_type = \"datetime\"", "read_only, sql_type = \"datetime(0)\"")
            .replace("sql_type = \"json\"", "read_only, sql_type = \"json\"")
            + "\npub fn descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> { vec![<BizEvolution as ryframe_sdk::ResourceModel>::descriptor()] }\n";
        let workspace = tempfile::Builder::new().prefix("imported-types-").tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests")).map_err(message)?;
        prepare(workspace.path(), &model)?;
        let root = workspace.path().join("evolution-business");
        let generated = generate(workspace.path())?;
        verify_current(&database, &root).await?;
        mysql::execute(&database, "RENAME TABLE biz_evolution TO biz_source_evolution").await?;
        let initial = generated.created.iter().find(|path| path.starts_with("migrations/")).ok_or("缺少初始迁移")?;
        execute_migration(&database, &root.join(initial)).await?;
        verify_current(&database, &root).await?;
        mysql::execute(&database, "INSERT INTO biz_evolution(id,code,memo,amount,occurred_at,payload,tiny_count,small_count,byte_count,short_count,count,large_count,price,exact_amount) VALUES(42,REPEAT('x',32),REPEAT('y',1024),2147483647,'2026-10-15 10:20:30.123',JSON_OBJECT('ok',true),-128,-32768,255,65535,4294967295,18446744073709551615,'90071992547409.123456','9999999999999999999999.999999')").await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_evolution WHERE id=42 AND CHAR_LENGTH(code)=32 AND CHAR_LENGTH(memo)=1024 AND MICROSECOND(occurred_at)=123000", 1).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_evolution WHERE id=42 AND tiny_count=-128 AND small_count=-32768 AND byte_count=255 AND short_count=65535 AND count=4294967295 AND large_count=18446744073709551615", 1).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_evolution WHERE id=42 AND code=REPEAT('X',32)", 0).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_evolution WHERE id=42 AND memo=REPEAT('Y',1024)", 1).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_source_evolution WHERE id=41 AND code='keep-source' AND amount=7", 1).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_evolution WHERE id=42 AND created_at IS NOT NULL AND updated_at IS NULL", 1).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_evolution WHERE id=42 AND CAST(price AS CHAR)='90071992547409.123456' AND CAST(exact_amount AS CHAR)='9999999999999999999999.999999'", 1).await?;
        mysql::execute(&database, "INSERT INTO biz_evolution(id) VALUES(43)").await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_evolution WHERE id=43 AND code=CONVERT(X'66726573685c70617468' USING utf8mb4) AND amount=7 AND large_count=18446744073709551615 AND created_at IS NOT NULL AND CAST(price AS CHAR)='0.125000'", 1).await?;
        mysql::execute(&database, "UPDATE biz_evolution SET updated_at='2000-01-01 00:00:00' WHERE id=42").await?;
        mysql::execute(&database, "UPDATE biz_evolution SET memo=CONCAT(memo,'!') WHERE id=42").await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_evolution WHERE id=42 AND updated_at>'2000-01-01 00:00:00'", 1).await?;
        let repeat = generate(workspace.path())?;
        assert!(repeat.created.is_empty() && repeat.updated.is_empty() && repeat.removed.is_empty());
        Ok(())
    }).await;
}

fn prepare(workspace: &Path, model: &str) -> Result<(), String> {
    let root = workspace.join("evolution-business");
    fs::create_dir_all(root.join("src/resources")).map_err(message)?;
    fs::write(
        workspace.join("Cargo.toml"),
        "[workspace]\nmembers = [\"evolution-business\"]\nresolver = \"3\"\n",
    )
    .map_err(message)?;
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ryframe-sdk")
        .to_string_lossy()
        .replace('\\', "/");
    fs::write(root.join("Cargo.toml"), format!("[package]\nname=\"evolution-business\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[package.metadata.ryframe]\nkind=\"business\"\nmodule=\"evolution\"\n[features]\ndefault=[]\ncatalog=[]\n[dependencies]\nryframe-sdk={{path={sdk:?}}}\n")).map_err(message)?;
    fs::write(root.join("src/lib.rs"), "pub mod resources;\npub fn resource_descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> { resources::descriptors() }\n").map_err(message)?;
    fs::write(root.join("src/resources/mod.rs"), model).map_err(message)?;
    let result = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(workspace)
        .output()
        .map_err(message)?;
    if !result.status.success() {
        return Err(format!(
            "业务夹具锁定依赖失败：{}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(())
}

fn generate(
    workspace: &Path,
) -> Result<ryframe_generator::business::BusinessGenerateReport, String> {
    generate_business_package(BusinessGenerateOptions {
        current_dir: workspace,
        package: "evolution-business",
        models: &[],
        write: true,
    })
    .map_err(message)
}

async fn execute_migration(database: &DatabaseConnection, path: &Path) -> Result<(), String> {
    let source = syn::parse_file(&fs::read_to_string(path).map_err(message)?).map_err(message)?;
    let constant = source
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Const(item) if item.ident == "CREATE_TABLE_DDL" || item.ident == "DDL" => {
                Some(item.expr.as_ref())
            }
            _ => None,
        })
        .ok_or("迁移没有同源 DDL 常量")?;
    let statements = match constant {
        syn::Expr::Reference(reference) => match reference.expr.as_ref() {
            syn::Expr::Array(array) => array.elems.iter().collect::<Vec<_>>(),
            _ => return Err("增量 DDL 常量不是字符串数组".into()),
        },
        value => vec![value],
    };
    for statement in statements {
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(sql),
            ..
        }) = statement
        else {
            return Err("迁移 DDL 不是静态字符串".into());
        };
        database
            .execute_unprepared(&sql.value())
            .await
            .map_err(mysql::db_error)?;
    }
    Ok(())
}

async fn verify_current(database: &DatabaseConnection, root: &Path) -> Result<(), String> {
    let history: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("src/generated/migrations/history.json")).map_err(message)?,
    )
    .map_err(message)?;
    let versions = history["resources"][0]["versions"]
        .as_array()
        .ok_or("缺少迁移历史")?;
    let expected = versions
        .last()
        .and_then(|version| version["canonical"].as_str())
        .ok_or("缺少最新结构")?;
    let actual = ryframe_db::schema::canonical_table_schema(database, "biz_evolution")
        .await
        .map_err(mysql::db_error)?;
    assert_eq!(actual, expected);
    Ok(())
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

const MODEL: &str = r#"#[derive(ryframe_sdk::ResourceModel)]
#[resource(name="evolution",title="业务演进",table="biz_evolution",database="control")]
pub struct Evolution {
    #[resource(primary_key, auto_increment)]
    pub id: i64,
    #[resource(unique)]
    pub code: String,
}
pub fn descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {
    vec![<Evolution as ryframe_sdk::ResourceModel>::descriptor()]
}
"#;
