use sea_orm::{DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement};

use super::catalog::{
    FenceCheckRow, FenceColumnRow, FenceConstraintRow, FenceIndexRow, TenantDataTableRow,
};
use super::{normalize_column_default, normalize_column_extra, schema_fingerprint_mismatch};
use ryframe_db::migration::normalize_check_clause;

pub(super) async fn verify_target_slot_schema(
    db: &DatabaseConnection,
    tables: &[TenantDataTableRow],
) -> Result<(), DbErr> {
    let slot = tables
        .iter()
        .find(|table| table.table_name == "biz_tenant_target_slot")
        .ok_or_else(|| schema_fingerprint_mismatch("target slot table"))?;
    if !slot.engine.eq_ignore_ascii_case("InnoDB")
        || !slot.character_set_name.eq_ignore_ascii_case("utf8mb4")
        || !slot
            .table_collation
            .eq_ignore_ascii_case("utf8mb4_general_ci")
    {
        return Err(schema_fingerprint_mismatch(
            "target slot engine/character-set/collation",
        ));
    }

    verify_columns(db).await?;
    verify_indexes(db).await?;
    verify_constraints(db).await?;
    Ok(())
}

async fn verify_columns(db: &DatabaseConnection) -> Result<(), DbErr> {
    let columns = FenceColumnRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT column_name AS `column_name`, column_type AS `column_type`, \
         is_nullable AS `is_nullable`, character_set_name AS `character_set_name`, \
         collation_name AS `collation_name`, column_key AS `column_key`, \
         column_default AS `column_default`, extra AS `extra`, \
         generation_expression AS `generation_expression` \
         FROM information_schema.columns WHERE table_schema = DATABASE() \
         AND table_name = 'biz_tenant_target_slot' ORDER BY ordinal_position",
    ))
    .all(db)
    .await?;
    let expected_columns = [
        (
            "slot_id",
            "tinyint unsigned",
            "NO",
            None,
            None,
            "PRI",
            None,
            "",
        ),
        (
            "tenant_id",
            "varchar(64)",
            "YES",
            Some("utf8mb4"),
            Some("utf8mb4_general_ci"),
            "",
            None,
            "",
        ),
        (
            "placement_generation",
            "bigint",
            "YES",
            None,
            None,
            "",
            None,
            "",
        ),
        (
            "switch_token",
            "varchar(64)",
            "YES",
            Some("ascii"),
            Some("ascii_bin"),
            "",
            None,
            "",
        ),
        (
            "updated_at",
            "datetime(6)",
            "NO",
            None,
            None,
            "",
            Some("current_timestamp(6)"),
            "on update current_timestamp(6)",
        ),
    ];
    if columns.len() != expected_columns.len() {
        return Err(schema_fingerprint_mismatch("target slot column count"));
    }
    for (actual, expected) in columns.iter().zip(expected_columns) {
        let (name, column_type, nullable, charset, collation, key, default, extra) = expected;
        if actual.column_name != name
            || actual.column_type.to_ascii_lowercase() != column_type
            || actual.is_nullable != nullable
            || actual.character_set_name.as_deref() != charset
            || actual.collation_name.as_deref() != collation
            || actual.column_key != key
            || normalize_column_default(actual.column_default.as_deref()) != default
            || normalize_column_extra(&actual.extra) != extra
            || !actual.generation_expression.trim().is_empty()
        {
            return Err(schema_fingerprint_mismatch("target slot column definition"));
        }
    }

    Ok(())
}

async fn verify_indexes(db: &DatabaseConnection) -> Result<(), DbErr> {
    let indexes = FenceIndexRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT index_name AS `index_name`, column_name AS `column_name`, \
         CAST(seq_in_index AS SIGNED) AS `seq_in_index`, \
         CAST(non_unique AS SIGNED) AS `non_unique`, index_type AS `index_type`, \
         CAST(sub_part AS SIGNED) AS `sub_part`, is_visible AS `is_visible` \
         FROM information_schema.statistics WHERE table_schema = DATABASE() \
         AND table_name = 'biz_tenant_target_slot' ORDER BY index_name, seq_in_index",
    ))
    .all(db)
    .await?;
    if indexes.len() != 1
        || indexes[0].index_name != "PRIMARY"
        || indexes[0].column_name != "slot_id"
        || indexes[0].seq_in_index != 1
        || indexes[0].non_unique != 0
        || !indexes[0].index_type.eq_ignore_ascii_case("BTREE")
        || indexes[0].sub_part.is_some()
        || indexes[0].is_visible != "YES"
    {
        return Err(schema_fingerprint_mismatch("target slot primary key"));
    }

    Ok(())
}

async fn verify_constraints(db: &DatabaseConnection) -> Result<(), DbErr> {
    let constraints = FenceConstraintRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT constraint_name AS `constraint_name`, constraint_type AS `constraint_type`, \
         enforced AS `enforced` \
         FROM information_schema.table_constraints \
         WHERE table_schema = DATABASE() AND table_name = 'biz_tenant_target_slot' \
         ORDER BY constraint_name",
    ))
    .all(db)
    .await?;
    let expected_constraints = [
        ("PRIMARY", "PRIMARY KEY"),
        ("ck_biz_tenant_target_slot_id", "CHECK"),
        ("ck_biz_tenant_target_slot_value", "CHECK"),
    ];
    if constraints.len() != expected_constraints.len()
        || expected_constraints.iter().any(|(name, kind)| {
            !constraints.iter().any(|constraint| {
                constraint.constraint_name == *name
                    && constraint.constraint_type == *kind
                    && constraint.enforced == "YES"
            })
        })
    {
        return Err(schema_fingerprint_mismatch("target slot constraints"));
    }
    let checks = FenceCheckRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT tc.constraint_name AS `constraint_name`, cc.check_clause AS `check_clause` \
         FROM information_schema.table_constraints tc \
         INNER JOIN information_schema.check_constraints cc \
           ON cc.constraint_schema = tc.constraint_schema \
          AND cc.constraint_name = tc.constraint_name \
         WHERE tc.table_schema = DATABASE() AND tc.table_name = 'biz_tenant_target_slot' \
           AND tc.constraint_type = 'CHECK'",
    ))
    .all(db)
    .await?;
    let slot_id = checks
        .iter()
        .find(|check| check.constraint_name == "ck_biz_tenant_target_slot_id")
        .map(|check| normalize_check_clause(&check.check_clause));
    let value = checks
        .iter()
        .find(|check| check.constraint_name == "ck_biz_tenant_target_slot_value")
        .map(|check| normalize_check_clause(&check.check_clause));
    let expected_slot_id = normalize_check_clause("`slot_id` = 1");
    let expected_value = normalize_check_clause(
        "(((`tenant_id` IS NULL) AND (`placement_generation` IS NULL) \
         AND (`switch_token` IS NULL)) OR ((`tenant_id` IS NOT NULL) \
         AND (`placement_generation` > 0) AND (`switch_token` IS NOT NULL)))",
    );
    if checks.len() != 2
        || slot_id.as_deref() != Some(expected_slot_id.as_str())
        || value.as_deref() != Some(expected_value.as_str())
    {
        return Err(schema_fingerprint_mismatch("target slot check constraints"));
    }
    Ok(())
}
