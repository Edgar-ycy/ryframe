use std::{fs, path::Path};

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use super::message;

#[derive(Debug, PartialEq, serde::Serialize)]
pub(super) struct TableImage {
    pub(super) rows: Vec<String>,
    schema: String,
    created: Vec<String>,
}

impl TableImage {
    pub(super) async fn capture(database: &DatabaseConnection) -> Result<Self, String> {
        Ok(Self {
            rows: rows(database).await?,
            schema: ryframe_db::schema::canonical_table_schema(database, "table1").await.map_err(message)?,
            created: images(database, "SELECT CAST(JSON_ARRAY(CAST(CREATE_TIME AS CHAR), AUTO_INCREMENT) AS CHAR) AS image FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='table1'").await?,
        })
    }

    pub(super) fn save(&self, path: &Path) -> Result<(), String> {
        fs::write(path, serde_json::to_vec_pretty(self).map_err(message)?).map_err(message)
    }
}

pub(super) async fn rows(database: &DatabaseConnection) -> Result<Vec<String>, String> {
    images(database, "SELECT CAST(JSON_ARRAY(CAST(id AS CHAR),name,code,CAST(price AS CHAR),DATE_FORMAT(created_at,'%Y-%m-%d %H:%i:%s.%f')) AS CHAR) AS image FROM table1 ORDER BY id").await
}

pub(super) async fn ledger(database: &DatabaseConnection) -> Result<Vec<String>, String> {
    if !ledger_exists(database).await? {
        return Ok(Vec::new());
    }
    images(database, "SELECT CAST(JSON_ARRAY(scope,module_name,migration_id,checksum,before_fingerprint,after_fingerprint,origin,tables_json,adoption_hash,DATE_FORMAT(applied_at,'%Y-%m-%d %H:%i:%s.%f')) AS CHAR) AS image FROM ryframe_business_migration WHERE scope='control' AND module_name='cli-import' ORDER BY migration_id").await
}

pub(super) async fn ledger_exists(database: &DatabaseConnection) -> Result<bool, String> {
    let row = database.query_one_raw(Statement::from_string(DbBackend::MySql,
        "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='ryframe_business_migration'"))
        .await.map_err(message)?.ok_or("缺少账本存在性结果")?;
    Ok(row.try_get::<i64>("", "value").map_err(message)? == 1)
}

pub(super) async fn require_origin(
    database: &DatabaseConnection,
    id: &str,
    origin: &str,
) -> Result<(), String> {
    let result = database.query_one_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT origin FROM ryframe_business_migration WHERE scope='control' AND module_name='cli-import' AND migration_id=?", [id.into()]))
        .await.map_err(message)?.ok_or("缺少正式业务迁移账本记录")?;
    assert_eq!(
        result.try_get::<String>("", "origin").map_err(message)?,
        origin
    );
    Ok(())
}

async fn images(database: &DatabaseConnection, sql: &str) -> Result<Vec<String>, String> {
    database
        .query_all_raw(Statement::from_string(DbBackend::MySql, sql))
        .await
        .map_err(message)?
        .into_iter()
        .map(|row| row.try_get::<String>("", "image").map_err(message))
        .collect()
}

pub(super) fn versions(business: &Path) -> Result<Vec<String>, String> {
    let history: serde_json::Value = serde_json::from_slice(
        &fs::read(business.join("src/generated/migrations/history.json")).map_err(message)?,
    )
    .map_err(message)?;
    let resources = history["resources"].as_array().ok_or("缺少迁移资源")?;
    if resources.len() != 1 || resources[0]["descriptor"]["table"] != "table1" {
        return Err("导入迁移历史必须只包含 table1".into());
    }
    resources[0]["versions"]
        .as_array()
        .ok_or("缺少迁移版本")?
        .iter()
        .map(|version| {
            version["id"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "迁移 ID 无效".into())
        })
        .collect()
}
