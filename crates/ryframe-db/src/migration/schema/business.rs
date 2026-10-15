use std::{
    collections::BTreeMap,
    sync::{OnceLock, RwLock},
};

use sea_orm::{DatabaseConnection, DbErr};

static BUSINESS_TABLES: OnceLock<RwLock<BTreeMap<String, String>>> = OnceLock::new();

/// 整批替换本进程启用的控制库业务结构；先预检，失败不改变旧登记。
pub fn register_business_tables<'a>(
    tables: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), DbErr> {
    let framework = super::expected::expected_schema()?;
    let mut candidate = BTreeMap::new();
    for (table, canonical) in tables {
        if table.is_empty()
            || !table
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(DbErr::Custom(format!("控制库业务表名非法：{table}")));
        }
        if framework.tables.contains_key(table)
            || matches!(table, "seaql_migrations" | "ryframe_business_migration")
        {
            return Err(DbErr::Custom(format!(
                "控制库业务表与框架保留表冲突：{table}"
            )));
        }
        if !crate::schema::validate_canonical_schema(table, canonical) {
            return Err(DbErr::Custom(format!(
                "控制库业务表 {table} 的结构描述非法或不完整；请重新生成当前业务模型"
            )));
        }
        if candidate
            .insert(table.to_owned(), canonical.to_owned())
            .is_some()
        {
            return Err(DbErr::Custom(format!("控制库业务表重复登记：{table}")));
        }
    }
    *BUSINESS_TABLES
        .get_or_init(|| RwLock::new(BTreeMap::new()))
        .write()
        .map_err(|_| DbErr::Custom("控制库业务结构登记锁已损坏，拒绝继续启动".into()))? = candidate;
    Ok(())
}

pub(super) fn registered_business_tables() -> Result<BTreeMap<String, String>, DbErr> {
    BUSINESS_TABLES.get().map_or_else(
        || Ok(BTreeMap::new()),
        |registry| {
            registry
                .read()
                .map(|tables| tables.clone())
                .map_err(|_| DbErr::Custom("控制库业务结构登记锁已损坏，拒绝跳过校验".into()))
        },
    )
}

/// 服务启动只读核对当前业务结构；不运行迁移、不登记账本、不修复已有数据。
pub async fn verify_business_schema(database: &DatabaseConnection) -> Result<(), DbErr> {
    for (table, expected) in registered_business_tables()? {
        let actual = crate::schema::canonical_table_schema(database, &table).await.map_err(|error| DbErr::Custom(format!("无法校验控制库业务表 {table}：{error}；本次结构校验未写入，请运行 cargo migrate -- control verify 核对缺失迁移与连接权限")))?;
        if actual != expected {
            let difference = crate::schema::describe_schema_difference(&expected, &actual);
            return Err(DbErr::Custom(format!(
                "控制库业务表 {table} 结构不一致：{difference}；本次结构校验未写入；请运行 cargo migrate -- control verify 并核对模型及迁移，不能自动重建或重放"
            )));
        }
    }
    Ok(())
}
