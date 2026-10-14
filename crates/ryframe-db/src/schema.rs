use std::fmt::Write as _;

use sea_orm::{DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement};

use crate::migration::normalize_check_clause;

#[derive(Debug, FromQueryResult)]
pub struct ColumnMetadata {
    pub column_name: String,
    pub column_type: String,
    pub is_nullable: String,
    pub character_set_name: Option<String>,
    pub collation_name: Option<String>,
    pub column_key: String,
    pub column_default: Option<String>,
    pub extra: String,
    pub generation_expression: String,
}

#[derive(Debug, FromQueryResult)]
pub struct IndexMetadata {
    pub index_name: String,
    pub column_name: String,
    pub seq_in_index: i64,
    pub non_unique: i64,
    pub index_type: String,
    pub sub_part: Option<i64>,
    pub is_visible: String,
}

#[derive(Debug, FromQueryResult)]
pub struct CheckMetadata {
    pub constraint_name: String,
    pub check_clause: String,
}

#[derive(Debug, FromQueryResult)]
pub struct TableMetadata {
    pub table_name: String,
    pub engine: String,
    pub character_set_name: String,
    pub table_collation: String,
}

#[derive(Debug, FromQueryResult)]
pub struct ConstraintMetadata {
    pub constraint_name: String,
    pub constraint_type: String,
    pub enforced: String,
}

#[derive(Debug, FromQueryResult)]
struct ForeignKeySchemaRow {
    constraint_name: String,
    column_name: String,
    ordinal_position: i64,
    referenced_table_schema: String,
    current_schema: String,
    referenced_table_name: String,
    referenced_column_name: String,
    update_rule: String,
    delete_rule: String,
}

/// 从 MySQL information_schema 只读读取明确指定表的完整、稳定结构。
/// 控制库、租户库和生成器共用同一规范化来源；调用方负责资源归属和运行范围。
pub async fn canonical_table_schema(
    db: &DatabaseConnection,
    table_name: &str,
) -> Result<String, DbErr> {
    if db.get_database_backend() != DbBackend::MySql {
        return Err(DbErr::Custom("表结构读取只支持 MySQL".into()));
    }
    if table_name.is_empty()
        || !table_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(DbErr::Custom(
            "表名必须是非空的字母、数字和下划线标识".into(),
        ));
    }
    let table = TableMetadata::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT t.table_name AS `table_name`, t.engine AS `engine`, \
                c.character_set_name AS `character_set_name`, \
                t.table_collation AS `table_collation` \
         FROM information_schema.tables t \
         INNER JOIN information_schema.collation_character_set_applicability c \
           ON c.collation_name = t.table_collation \
         WHERE t.table_schema = DATABASE() AND t.table_type = 'BASE TABLE' \
           AND t.table_name = ? LIMIT 1",
        [table_name.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| DbErr::Custom(format!("未找到普通表：{table_name}")))?;

    let columns = ColumnMetadata::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT column_name AS `column_name`, column_type AS `column_type`, \
                is_nullable AS `is_nullable`, character_set_name AS `character_set_name`, \
                collation_name AS `collation_name`, column_key AS `column_key`, \
                column_default AS `column_default`, extra AS `extra`, \
                generation_expression AS `generation_expression` \
         FROM information_schema.columns WHERE table_schema = DATABASE() \
           AND table_name = ? ORDER BY ordinal_position",
        [table_name.into()],
    ))
    .all(db)
    .await?;
    let indexes = IndexMetadata::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT index_name AS `index_name`, column_name AS `column_name`, \
         CAST(seq_in_index AS SIGNED) AS `seq_in_index`, \
         CAST(non_unique AS SIGNED) AS `non_unique`, index_type AS `index_type`, \
         CAST(sub_part AS SIGNED) AS `sub_part`, is_visible AS `is_visible` \
         FROM information_schema.statistics WHERE table_schema = DATABASE() \
           AND table_name = ? ORDER BY index_name, seq_in_index",
        [table_name.into()],
    ))
    .all(db)
    .await?;
    let constraints = ConstraintMetadata::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT constraint_name AS `constraint_name`, constraint_type AS `constraint_type`, \
                enforced AS `enforced` \
         FROM information_schema.table_constraints WHERE table_schema = DATABASE() \
           AND table_name = ? ORDER BY constraint_name",
        [table_name.into()],
    ))
    .all(db)
    .await?;
    let checks = CheckMetadata::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT tc.constraint_name AS `constraint_name`, cc.check_clause AS `check_clause` \
         FROM information_schema.table_constraints tc \
         INNER JOIN information_schema.check_constraints cc \
           ON cc.constraint_schema = tc.constraint_schema \
          AND cc.constraint_name = tc.constraint_name \
         WHERE tc.table_schema = DATABASE() AND tc.table_name = ? \
           AND tc.constraint_type = 'CHECK' ORDER BY tc.constraint_name",
        [table_name.into()],
    ))
    .all(db)
    .await?;
    let foreign_keys = ForeignKeySchemaRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT k.constraint_name AS `constraint_name`, k.column_name AS `column_name`, \
                CAST(k.ordinal_position AS SIGNED) AS `ordinal_position`, \
                k.referenced_table_schema AS `referenced_table_schema`, \
                DATABASE() AS `current_schema`, \
                k.referenced_table_name AS `referenced_table_name`, \
                k.referenced_column_name AS `referenced_column_name`, \
                r.update_rule AS `update_rule`, r.delete_rule AS `delete_rule` \
         FROM information_schema.key_column_usage k \
         INNER JOIN information_schema.referential_constraints r \
           ON r.constraint_schema = k.constraint_schema \
          AND r.table_name = k.table_name \
          AND r.constraint_name = k.constraint_name \
         WHERE k.table_schema = DATABASE() AND k.table_name = ? \
           AND k.referenced_table_name IS NOT NULL \
         ORDER BY k.constraint_name, k.ordinal_position",
        [table_name.into()],
    ))
    .all(db)
    .await?;
    ensure_local_foreign_key_schemas(&foreign_keys)?;

    Ok(render_table_schema(
        table,
        columns,
        indexes,
        constraints,
        checks,
        foreign_keys,
    ))
}

fn ensure_local_foreign_key_schemas(foreign_keys: &[ForeignKeySchemaRow]) -> Result<(), DbErr> {
    for foreign_key in foreign_keys {
        ensure_local_foreign_key_schema(
            &foreign_key.current_schema,
            &foreign_key.referenced_table_schema,
        )?;
    }
    Ok(())
}

pub fn ensure_local_foreign_key_schema(
    current_schema: &str,
    referenced_schema: &str,
) -> Result<(), DbErr> {
    if referenced_schema == current_schema {
        Ok(())
    } else {
        Err(DbErr::Custom(
            "外键必须位于同一目标数据库；跨库外键需要显式业务扩展".into(),
        ))
    }
}

fn render_table_schema(
    table: TableMetadata,
    columns: Vec<ColumnMetadata>,
    indexes: Vec<IndexMetadata>,
    constraints: Vec<ConstraintMetadata>,
    checks: Vec<CheckMetadata>,
    foreign_keys: Vec<ForeignKeySchemaRow>,
) -> String {
    let mut canonical = format!(
        "v2|table={:?}|engine={:?}|charset={:?}|collation={:?}|columns=[",
        table.table_name,
        table.engine.to_ascii_lowercase(),
        table.character_set_name.to_ascii_lowercase(),
        table.table_collation.to_ascii_lowercase(),
    );
    for column in columns {
        write!(
            canonical,
            "{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?};",
            column.column_name,
            column.column_type.to_ascii_lowercase(),
            column.is_nullable,
            column
                .character_set_name
                .map(|value| value.to_ascii_lowercase()),
            column
                .collation_name
                .map(|value| value.to_ascii_lowercase()),
            column.column_key,
            normalize_column_default(column.column_default.as_deref()),
            normalize_column_extra(&column.extra),
            column.generation_expression,
        )
        .expect("写入 String 不会失败");
    }
    canonical.push_str("]|indexes=[");
    for index in indexes {
        write!(
            canonical,
            "{:?}:{:?}:{}:{}:{:?}:{:?}:{:?};",
            index.index_name,
            index.column_name,
            index.seq_in_index,
            index.non_unique,
            index.index_type.to_ascii_lowercase(),
            index.sub_part,
            index.is_visible,
        )
        .expect("写入 String 不会失败");
    }
    canonical.push_str("]|constraints=[");
    for constraint in constraints {
        write!(
            canonical,
            "{:?}:{:?}:{:?};",
            constraint.constraint_name, constraint.constraint_type, constraint.enforced,
        )
        .expect("写入 String 不会失败");
    }
    canonical.push_str("]|checks=[");
    for check in checks {
        write!(
            canonical,
            "{:?}:{:?};",
            check.constraint_name,
            normalize_check_clause(&check.check_clause),
        )
        .expect("写入 String 不会失败");
    }
    canonical.push_str("]|foreign_keys=[");
    for foreign_key in foreign_keys {
        write!(
            canonical,
            "{:?}:{:?}:{}:local:{:?}:{:?}:{:?}:{:?};",
            foreign_key.constraint_name,
            foreign_key.column_name,
            foreign_key.ordinal_position,
            foreign_key.referenced_table_name,
            foreign_key.referenced_column_name,
            foreign_key.update_rule,
            foreign_key.delete_rule,
        )
        .expect("写入 String 不会失败");
    }
    canonical.push(']');
    canonical
}

pub fn normalize_column_default(value: Option<&str>) -> Option<&str> {
    value.map(|value| {
        if value.eq_ignore_ascii_case("current_timestamp(6)") {
            "current_timestamp(6)"
        } else {
            value
        }
    })
}

pub fn normalize_column_extra(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .split_whitespace()
        .filter(|part| *part != "default_generated")
        .collect::<Vec<_>>()
        .join(" ")
}
