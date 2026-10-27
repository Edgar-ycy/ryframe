use std::collections::BTreeMap;

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, Value};
use serde_json::Value as Json;

/// 全部控制库基表都参与比较，空表也保留；错误只报告表名和差异类别。
pub(super) async fn compare(
    imported: &DatabaseConnection,
    migrated: &DatabaseConnection,
) -> Result<(), String> {
    let left = inventory(imported).await?;
    let right = inventory(migrated).await?;
    for table in left.keys().chain(right.keys()) {
        let (Some(left), Some(right)) = (left.get(table), right.get(table)) else {
            return Err(format!("表 {table}：控制库基表集合不一致"));
        };
        if left.len() != right.len() {
            return Err(format!("表 {table}：初始化行数不一致"));
        }
        if left != right {
            return Err(format!("表 {table}：初始化规范行数据不一致"));
        }
    }
    Ok(())
}

async fn inventory(database: &DatabaseConnection) -> Result<BTreeMap<String, Vec<String>>, String> {
    let metadata = database
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT c.TABLE_NAME AS table_name, c.COLUMN_NAME AS column_name \
             FROM information_schema.columns c JOIN information_schema.tables t \
             ON t.TABLE_SCHEMA=c.TABLE_SCHEMA AND t.TABLE_NAME=c.TABLE_NAME \
             WHERE c.TABLE_SCHEMA=DATABASE() AND t.TABLE_TYPE='BASE TABLE' \
             ORDER BY c.TABLE_NAME, c.ORDINAL_POSITION",
        ))
        .await
        .map_err(|_| "控制库基表元数据：读取失败".to_owned())?;
    let mut tables: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in metadata {
        let table = row
            .try_get::<String>("", "table_name")
            .map_err(|_| "控制库基表元数据：表名无效".to_owned())?;
        let column = row
            .try_get("", "column_name")
            .map_err(|_| format!("表 {table}：列名元数据无效"))?;
        tables.entry(table).or_default().push(column);
    }
    if tables.is_empty() {
        return Err("控制库基表集合为空，不能判定初始化等价".into());
    }
    let mut inventory = BTreeMap::new();
    for (table, columns) in tables {
        validate_generated_values(database, &table, &columns).await?;
        let (sql, keys) = projection(&table, &columns);
        let rows = database
            .query_all_raw(Statement::from_sql_and_values(DbBackend::MySql, sql, keys))
            .await
            .map_err(|_| format!("表 {table}：初始化行投影读取失败"))?;
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            let item: String = row
                .try_get("", "item")
                .map_err(|_| format!("表 {table}：初始化行投影类型无效"))?;
            let json = serde_json::from_str(&item)
                .map_err(|_| format!("表 {table}：初始化行 JSON 无效"))?;
            let canonical = serde_json::to_string(&canonical(json))
                .map_err(|_| format!("表 {table}：初始化行 JSON 规范化失败"))?;
            values.push(canonical);
        }
        values.sort();
        inventory.insert(table, values);
    }
    Ok(inventory)
}

fn identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

fn projection(table: &str, columns: &[String]) -> (String, Vec<Value>) {
    let mut fields = Vec::with_capacity(columns.len());
    let mut keys = Vec::with_capacity(columns.len());
    for column in columns {
        let quoted = identifier(column);
        // 仅清单内的非确定值归一为存在标志；密码哈希及其他列保留完整值。
        let value = if generated_time(table, column)
            || (table == "seaql_migrations" && column == "applied_at")
            || (table == "sys_outbox_event" && column == "id")
        {
            format!("IF({quoted} IS NULL, NULL, 1)")
        } else {
            quoted
        };
        fields.push(format!("?, {value}"));
        keys.push(column.clone().into());
    }
    (
        format!(
            "SELECT CAST(JSON_OBJECT({}) AS CHAR) AS item FROM {}",
            fields.join(", "),
            identifier(table),
        ),
        keys,
    )
}

fn generated_time(table: &str, column: &str) -> bool {
    matches!(column, "created_at" | "updated_at")
        || matches!(
            (table, column),
            ("sys_product_plan_version", "published_at")
                | ("sys_job_schedule", "next_run_at")
                | ("sys_outbox_event", "available_at")
        )
}

async fn validate_generated_values(
    database: &DatabaseConnection,
    table: &str,
    columns: &[String],
) -> Result<(), String> {
    let mut invalid = Vec::new();
    for column in columns {
        let quoted = identifier(column);
        if generated_time(table, column) {
            // 非 NULL 日期必须确为有效日历日期，不能把两边的零日期归一成相等。
            invalid.push(format!(
                "({quoted} IS NOT NULL AND NOT COALESCE(\
                 YEAR({quoted}) BETWEEN 1000 AND 9999 \
                 AND MONTH({quoted}) BETWEEN 1 AND 12 \
                 AND DAYOFMONTH({quoted}) BETWEEN 1 AND DAYOFMONTH(LAST_DAY({quoted})), FALSE))"
            ));
        } else if (table == "seaql_migrations" && column == "applied_at")
            || (table == "sys_outbox_event" && column == "id")
        {
            invalid.push(format!("({quoted} IS NULL OR {quoted} <= 0)"));
        }
    }
    if invalid.is_empty() {
        return Ok(());
    }
    let uniqueness = if table == "sys_outbox_event" {
        "COUNT(*) - COUNT(DISTINCT `id`)"
    } else {
        "0"
    };
    let sql = format!(
        "SELECT CAST(COUNT(CASE WHEN {} THEN 1 END) AS UNSIGNED) AS invalid_values, \
         CAST({uniqueness} AS UNSIGNED) AS duplicate_ids FROM {}",
        invalid.join(" OR "),
        identifier(table),
    );
    let row = database
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql))
        .await
        .map_err(|_| format!("表 {table}：非确定字段有效性读取失败"))?
        .ok_or_else(|| format!("表 {table}：非确定字段有效性结果缺失"))?;
    for category in ["invalid_values", "duplicate_ids"] {
        let count: u64 = row
            .try_get("", category)
            .map_err(|_| format!("表 {table}：{category} 计数无效"))?;
        if count != 0 {
            return Err(format!("表 {table}：非确定字段校验失败（{category}）"));
        }
    }
    Ok(())
}

fn canonical(value: Json) -> Json {
    match value {
        Json::Object(fields) => {
            let ordered: BTreeMap<_, _> = fields.into_iter().collect();
            Json::Object(
                ordered
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect(),
            )
        }
        Json::Array(values) => Json::Array(values.into_iter().map(canonical).collect()),
        value => value,
    }
}
