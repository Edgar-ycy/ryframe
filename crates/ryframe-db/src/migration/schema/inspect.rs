use std::collections::BTreeMap;

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, TryGetable};

use super::{
    normalize::{
        normalize_action, normalize_actual_extra, normalize_column_type, normalize_default,
        normalize_generation_expression, normalize_identifier,
    },
    types::{ActualColumn, ActualForeignKey, ActualIndex, ActualTable},
};

#[cfg(any(feature = "migration", test))]
const USER_TABLES_SQL: &str = concat!(
    "SELECT TABLE_NAME AS `table_name` FROM information_schema.TABLES ",
    "WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' ",
    "AND TABLE_NAME <> 'seaql_migrations' ORDER BY TABLE_NAME",
);

const ACTUAL_TABLES_SQL: &str = concat!(
    "SELECT t.TABLE_NAME AS `table_name`, t.ENGINE AS `engine`, ",
    "c.CHARACTER_SET_NAME AS `character_set_name`, ",
    "t.TABLE_COLLATION AS `table_collation` ",
    "FROM information_schema.TABLES t ",
    "JOIN information_schema.COLLATION_CHARACTER_SET_APPLICABILITY c ",
    "ON c.COLLATION_NAME = t.TABLE_COLLATION ",
    "WHERE t.TABLE_SCHEMA = DATABASE() AND t.TABLE_TYPE = 'BASE TABLE'",
);

const ACTUAL_COLUMNS_SQL: &str = concat!(
    "SELECT TABLE_NAME AS `table_name`, COLUMN_NAME AS `column_name`, ",
    "COLUMN_TYPE AS `column_type`, IS_NULLABLE AS `is_nullable`, ",
    "COLUMN_DEFAULT AS `column_default`, EXTRA AS `extra`, ",
    "CHARACTER_SET_NAME AS `character_set_name`, ",
    "COLLATION_NAME AS `collation_name`, ",
    "GENERATION_EXPRESSION AS `generation_expression` ",
    "FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE()",
);

const ACTUAL_INDEXES_SQL: &str = concat!(
    "SELECT TABLE_NAME AS `table_name`, INDEX_NAME AS `index_name`, ",
    "CAST(NON_UNIQUE AS SIGNED) AS `non_unique`, ",
    "CAST(SEQ_IN_INDEX AS SIGNED) AS `seq_in_index`, ",
    "COALESCE(COLUMN_NAME, EXPRESSION) AS `column_expression`, ",
    "CAST(SUB_PART AS SIGNED) AS `sub_part`, INDEX_TYPE AS `index_type`, ",
    "IS_VISIBLE AS `is_visible` ",
    "FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = DATABASE() ",
    "ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX",
);

const ACTUAL_FOREIGN_KEYS_SQL: &str = concat!(
    "SELECT k.TABLE_NAME AS `table_name`, k.CONSTRAINT_NAME AS `constraint_name`, ",
    "CAST(k.ORDINAL_POSITION AS SIGNED) AS `ordinal_position`, ",
    "k.COLUMN_NAME AS `column_name`, ",
    "k.REFERENCED_TABLE_NAME AS `referenced_table_name`, ",
    "k.REFERENCED_COLUMN_NAME AS `referenced_column_name`, ",
    "r.UPDATE_RULE AS `update_rule`, r.DELETE_RULE AS `delete_rule` ",
    "FROM information_schema.KEY_COLUMN_USAGE k ",
    "JOIN information_schema.REFERENTIAL_CONSTRAINTS r ",
    "ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA ",
    "AND r.TABLE_NAME = k.TABLE_NAME ",
    "AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME ",
    "WHERE k.CONSTRAINT_SCHEMA = DATABASE() ",
    "AND k.REFERENCED_TABLE_NAME IS NOT NULL ",
    "ORDER BY k.TABLE_NAME, k.CONSTRAINT_NAME, k.ORDINAL_POSITION",
);

#[cfg(feature = "migration")]
pub(crate) async fn user_tables<C>(db: &C) -> Result<Vec<String>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            USER_TABLES_SQL.to_owned(),
        ))
        .await?;
    let mut tables = Vec::with_capacity(rows.len());
    for row in rows {
        tables.push(String::try_get_by_index(&row, 0)?);
    }
    Ok(tables)
}

pub(super) async fn actual_tables<C>(db: &C) -> Result<BTreeMap<String, ActualTable>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            ACTUAL_TABLES_SQL.to_owned(),
        ))
        .await?;
    let mut tables = BTreeMap::new();
    for row in rows {
        let table = String::try_get_by_index(&row, 0)?;
        if is_tenant_data_object(&table) {
            continue;
        }
        let engine = normalize_identifier(&String::try_get_by_index(&row, 1)?);
        let character_set = normalize_identifier(&String::try_get_by_index(&row, 2)?);
        let collation = normalize_identifier(&String::try_get_by_index(&row, 3)?);
        tables.insert(
            table,
            ActualTable {
                engine,
                character_set,
                collation,
            },
        );
    }
    Ok(tables)
}

pub(super) async fn actual_columns<C>(
    db: &C,
) -> Result<BTreeMap<(String, String), ActualColumn>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            ACTUAL_COLUMNS_SQL.to_owned(),
        ))
        .await?;
    let mut columns = BTreeMap::new();
    for row in rows {
        let table = String::try_get_by_index(&row, 0)?;
        let column = String::try_get_by_index(&row, 1)?;
        let column_type = String::try_get_by_index(&row, 2)?;
        let nullable = String::try_get_by_index(&row, 3)? == "YES";
        let default =
            Option::<String>::try_get_by_index(&row, 4)?.map(|value| normalize_default(&value));
        let extra = normalize_actual_extra(&String::try_get_by_index(&row, 5)?);
        let character_set =
            Option::<String>::try_get_by_index(&row, 6)?.map(|value| normalize_identifier(&value));
        let collation =
            Option::<String>::try_get_by_index(&row, 7)?.map(|value| normalize_identifier(&value));
        let generation_expression =
            normalize_generation_expression(&String::try_get_by_index(&row, 8)?);
        columns.insert(
            (table, column),
            ActualColumn {
                column_type: normalize_column_type(&column_type),
                nullable,
                default,
                extra,
                character_set,
                collation,
                generation_expression,
            },
        );
    }
    Ok(columns)
}

pub(super) async fn actual_indexes<C>(
    db: &C,
) -> Result<BTreeMap<(String, String), ActualIndex>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            ACTUAL_INDEXES_SQL.to_owned(),
        ))
        .await?;
    let mut indexes = BTreeMap::<(String, String), ActualIndex>::new();
    for row in rows {
        let table = String::try_get_by_index(&row, 0)?;
        let name = String::try_get_by_index(&row, 1)?;
        let non_unique = i64::try_get_by_index(&row, 2)?;
        let sequence = i64::try_get_by_index(&row, 3)?;
        let column = String::try_get_by_index(&row, 4)?;
        let prefix_length = Option::<i64>::try_get_by_index(&row, 5)?;
        let index_type = normalize_identifier(&String::try_get_by_index(&row, 6)?);
        let visible = String::try_get_by_index(&row, 7)? == "YES";
        let entry = indexes.entry((table, name)).or_default();
        entry.unique = non_unique == 0;
        entry.index_type = index_type;
        entry.visible = visible;
        entry.columns.push((sequence, column, prefix_length));
    }
    Ok(indexes)
}

pub(super) async fn actual_foreign_keys<C>(
    db: &C,
) -> Result<BTreeMap<(String, String), ActualForeignKey>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            ACTUAL_FOREIGN_KEYS_SQL.to_owned(),
        ))
        .await?;
    let mut foreign_keys = BTreeMap::<(String, String), ActualForeignKey>::new();
    for row in rows {
        let table = String::try_get_by_index(&row, 0)?;
        let name = String::try_get_by_index(&row, 1)?;
        let sequence = i64::try_get_by_index(&row, 2)?;
        let column = String::try_get_by_index(&row, 3)?;
        let referenced_table = String::try_get_by_index(&row, 4)?;
        let referenced_column = String::try_get_by_index(&row, 5)?;
        let update_rule = normalize_action(&String::try_get_by_index(&row, 6)?);
        let delete_rule = normalize_action(&String::try_get_by_index(&row, 7)?);
        let entry = foreign_keys.entry((table, name)).or_default();
        entry.referenced_table = referenced_table;
        entry.update_rule = update_rule;
        entry.delete_rule = delete_rule;
        entry.columns.push((sequence, column));
        entry.referenced_columns.push((sequence, referenced_column));
    }
    Ok(foreign_keys)
}

fn is_tenant_data_object(table: &str) -> bool {
    table == "seaql_tenant_data_migrations" || table.starts_with("biz_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_queries_do_not_contain_escape_artifacts() {
        for query in [
            USER_TABLES_SQL,
            ACTUAL_TABLES_SQL,
            ACTUAL_COLUMNS_SQL,
            ACTUAL_INDEXES_SQL,
            ACTUAL_FOREIGN_KEYS_SQL,
        ] {
            assert!(!query.contains('\\'));
            assert!(!query.contains('\n'));
            assert!(!query.contains('\r'));
        }
    }
}
