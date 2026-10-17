use ryframe_db::DbResultExt;
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{DatabaseConnection, FromQueryResult, Statement};

use crate::import::ForeignKeyInfo;

#[derive(Debug, FromQueryResult)]
struct ForeignKeyColumnRow {
    constraint_name: String,
    column_name: String,
    ordinal_position: i64,
    referenced_table_schema: String,
    current_schema: String,
    referenced_table_name: String,
    referenced_column_name: String,
    update_rule: String,
    delete_rule: String,
    match_option: String,
}

pub(super) async fn query_foreign_keys(
    db: &DatabaseConnection,
    table_name: &str,
) -> AppResult<Vec<ForeignKeyInfo>> {
    let rows = ForeignKeyColumnRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT k.constraint_name AS `constraint_name`, k.column_name AS `column_name`, \
         CAST(k.ordinal_position AS SIGNED) AS `ordinal_position`, \
         k.referenced_table_schema AS `referenced_table_schema`, \
         DATABASE() AS `current_schema`, \
         k.referenced_table_name AS `referenced_table_name`, \
         k.referenced_column_name AS `referenced_column_name`, \
         r.update_rule AS `update_rule`, r.delete_rule AS `delete_rule`, \
         r.match_option AS `match_option` \
         FROM information_schema.key_column_usage k \
         LEFT JOIN information_schema.referential_constraints r \
           ON r.constraint_schema = k.constraint_schema \
          AND r.table_name = k.table_name AND r.constraint_name = k.constraint_name \
         WHERE k.table_schema = DATABASE() AND k.table_name = ? \
           AND k.referenced_table_name IS NOT NULL \
         ORDER BY k.constraint_name, k.ordinal_position",
        [table_name.into()],
    ))
    .all(db)
    .await
    .db_context("查询外键结构失败")?;
    collect_foreign_keys(table_name, rows)
}

fn collect_foreign_keys(
    table_name: &str,
    rows: Vec<ForeignKeyColumnRow>,
) -> AppResult<Vec<ForeignKeyInfo>> {
    let mut foreign_keys: Vec<ForeignKeyInfo> = Vec::new();
    for row in rows {
        ryframe_db::schema::ensure_local_foreign_key_schema(
            &row.current_schema,
            &row.referenced_table_schema,
        )
        .map_err(|_| {
            AppError::Validation(format!(
                "表 {table_name} 的外键 {} 跨 schema 引用 {}，已拒绝生成",
                row.constraint_name, row.referenced_table_schema
            ))
        })?;
        let invalid_rule = |clause: &str, error: String| {
            AppError::Validation(format!(
                "表 {table_name} 的外键 {}，{clause}：{error}",
                row.constraint_name
            ))
        };
        let update_rule = row
            .update_rule
            .parse()
            .map_err(|error| invalid_rule("ON UPDATE", error))?;
        let delete_rule = row
            .delete_rule
            .parse()
            .map_err(|error| invalid_rule("ON DELETE", error))?;
        let match_option = row
            .match_option
            .parse()
            .map_err(|error| invalid_rule("MATCH", error))?;
        if let Some(foreign_key) = foreign_keys.last_mut()
            && foreign_key.name == row.constraint_name
        {
            if foreign_key.referenced_table != row.referenced_table_name
                || foreign_key.update_rule != update_rule
                || foreign_key.delete_rule != delete_rule
                || foreign_key.match_option != match_option
                || usize::try_from(row.ordinal_position).ok() != Some(foreign_key.columns.len() + 1)
            {
                return Err(AppError::Validation(format!(
                    "表 {table_name} 的外键 {} 引用结构、动作或列顺序不一致",
                    row.constraint_name
                )));
            }
            foreign_key.columns.push(row.column_name);
            foreign_key
                .referenced_columns
                .push(row.referenced_column_name);
        } else {
            if row.ordinal_position != 1 {
                return Err(AppError::Validation(format!(
                    "表 {table_name} 的外键 {} 缺少首列，拒绝保留不完整结构",
                    row.constraint_name
                )));
            }
            foreign_keys.push(ForeignKeyInfo {
                name: row.constraint_name,
                columns: vec![row.column_name],
                referenced_table: row.referenced_table_name,
                referenced_columns: vec![row.referenced_column_name],
                update_rule,
                delete_rule,
                match_option,
            });
        }
    }
    Ok(foreign_keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(sequence: i64) -> ForeignKeyColumnRow {
        ForeignKeyColumnRow {
            constraint_name: "fk_child_parent".into(),
            column_name: format!("child_{sequence}"),
            ordinal_position: sequence,
            referenced_table_schema: "same_schema".into(),
            current_schema: "same_schema".into(),
            referenced_table_name: "parent".into(),
            referenced_column_name: format!("parent_{sequence}"),
            update_rule: "CASCADE".into(),
            delete_rule: "SET NULL".into(),
            match_option: "NONE".into(),
        }
    }

    #[test]
    fn schema_identity_and_unknown_rules_fail_closed() {
        for (field, value, expected) in [
            ("schema", "SAME_SCHEMA", "跨 schema"),
            ("update", "FUTURE ACTION", "ON UPDATE"),
            ("delete", "FUTURE ACTION", "ON DELETE"),
            ("match", "FUTURE MATCH", "MATCH"),
        ] {
            let mut source = row(1);
            match field {
                "schema" => source.referenced_table_schema = value.into(),
                "update" => source.update_rule = value.into(),
                "delete" => source.delete_rule = value.into(),
                "match" => source.match_option = value.into(),
                _ => unreachable!("测试字段固定"),
            }
            let error = collect_foreign_keys("child", vec![source])
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("fk_child_parent") && error.contains(expected),
                "{error}"
            );
        }
    }

    #[test]
    fn composite_rows_must_agree_and_keep_contiguous_order() {
        let keys = collect_foreign_keys("child", vec![row(1), row(2)]).unwrap();
        assert_eq!(keys[0].columns, ["child_1", "child_2"]);
        assert_eq!(keys[0].referenced_columns, ["parent_1", "parent_2"]);
        for (index, mut second) in [row(2), row(2), row(2), row(2), row(3)]
            .into_iter()
            .enumerate()
        {
            match index {
                0 => second.referenced_table_name = "other_parent".into(),
                1 => second.update_rule = "RESTRICT".into(),
                2 => second.delete_rule = "RESTRICT".into(),
                3 => second.match_option = "FULL".into(),
                _ => {}
            }
            assert!(collect_foreign_keys("child", vec![row(1), second]).is_err());
        }
        assert!(collect_foreign_keys("child", vec![row(2)]).is_err());
    }

    #[test]
    fn different_constraints_keep_their_own_rules() {
        let mut second = row(1);
        second.constraint_name = "fk_other_parent".into();
        second.update_rule = "RESTRICT".into();
        second.delete_rule = "CASCADE".into();
        let keys = collect_foreign_keys("child", vec![row(1), second]).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(
            keys[0].update_rule,
            crate::import::ForeignKeyAction::Cascade
        );
        assert_eq!(
            keys[0].delete_rule,
            crate::import::ForeignKeyAction::SetNull
        );
        assert_eq!(
            keys[1].update_rule,
            crate::import::ForeignKeyAction::Restrict
        );
        assert_eq!(
            keys[1].delete_rule,
            crate::import::ForeignKeyAction::Cascade
        );
    }
}
