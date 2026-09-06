use super::*;

pub(super) async fn verify_fence_columns(db: &DatabaseConnection) -> Result<(), DbErr> {
    let columns = FenceColumnRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT column_name AS `column_name`, column_type AS `column_type`, \
         is_nullable AS `is_nullable`, character_set_name AS `character_set_name`, \
         collation_name AS `collation_name`, column_key AS `column_key`, \
         column_default AS `column_default`, extra AS `extra`, \
         generation_expression AS `generation_expression` \
         FROM information_schema.columns WHERE table_schema = DATABASE() \
         AND table_name = 'biz_tenant_fence' ORDER BY ordinal_position",
    ))
    .all(db)
    .await?;
    let expected_columns = [
        (
            "tenant_id",
            "varchar(64)",
            Some("utf8mb4"),
            Some("utf8mb4_general_ci"),
            "PRI",
            None,
            "",
        ),
        (
            "target_key",
            "varchar(64)",
            Some("ascii"),
            Some("ascii_bin"),
            "",
            None,
            "",
        ),
        ("placement_generation", "bigint", None, None, "", None, ""),
        (
            "state",
            "varchar(16)",
            Some("ascii"),
            Some("ascii_bin"),
            "MUL",
            None,
            "",
        ),
        (
            "switch_token",
            "varchar(64)",
            Some("ascii"),
            Some("ascii_bin"),
            "",
            None,
            "",
        ),
        (
            "updated_at",
            "datetime(6)",
            None,
            None,
            "",
            Some("current_timestamp(6)"),
            "on update current_timestamp(6)",
        ),
    ];
    if columns.len() != expected_columns.len() {
        return Err(schema_fingerprint_mismatch("fence column count"));
    }
    for (actual, expected) in columns.iter().zip(expected_columns) {
        let (name, column_type, charset, collation, column_key, default, extra) = expected;
        if actual.column_name != name
            || actual.column_type.to_ascii_lowercase() != column_type
            || actual.is_nullable != "NO"
            || actual.character_set_name.as_deref() != charset
            || actual.collation_name.as_deref() != collation
            || actual.column_key != column_key
            || normalize_column_default(actual.column_default.as_deref()) != default
            || normalize_column_extra(&actual.extra) != extra
            || !actual.generation_expression.trim().is_empty()
        {
            return Err(schema_fingerprint_mismatch("fence column definition"));
        }
    }

    Ok(())
}

pub(super) async fn verify_fence_indexes(db: &DatabaseConnection) -> Result<(), DbErr> {
    let indexes = FenceIndexRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT index_name AS `index_name`, column_name AS `column_name`, \
         CAST(seq_in_index AS SIGNED) AS `seq_in_index`, \
         CAST(non_unique AS SIGNED) AS `non_unique`, index_type AS `index_type`, \
         CAST(sub_part AS SIGNED) AS `sub_part`, is_visible AS `is_visible` \
         FROM information_schema.statistics WHERE table_schema = DATABASE() \
         AND table_name = 'biz_tenant_fence' \
         ORDER BY index_name, seq_in_index",
    ))
    .all(db)
    .await?;
    let primary = indexes
        .iter()
        .filter(|index| index.index_name == "PRIMARY")
        .collect::<Vec<_>>();
    let state_index = indexes
        .iter()
        .filter(|index| index.index_name == "idx_biz_tenant_fence_state")
        .collect::<Vec<_>>();
    if indexes.len() != 3
        || primary.len() != 1
        || primary[0].column_name != "tenant_id"
        || primary[0].seq_in_index != 1
        || primary[0].non_unique != 0
        || !primary[0].index_type.eq_ignore_ascii_case("BTREE")
        || primary[0].sub_part.is_some()
        || primary[0].is_visible != "YES"
        || state_index.len() != 2
        || state_index[0].column_name != "state"
        || state_index[0].seq_in_index != 1
        || state_index[1].column_name != "tenant_id"
        || state_index[1].seq_in_index != 2
        || state_index.iter().any(|index| {
            index.non_unique != 1
                || !index.index_type.eq_ignore_ascii_case("BTREE")
                || index.sub_part.is_some()
                || index.is_visible != "YES"
        })
    {
        return Err(schema_fingerprint_mismatch("fence primary/key index"));
    }

    Ok(())
}

pub(super) async fn verify_fence_constraints(db: &DatabaseConnection) -> Result<(), DbErr> {
    let constraints = FenceConstraintRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT constraint_name AS `constraint_name`, constraint_type AS `constraint_type`, \
         enforced AS `enforced` \
         FROM information_schema.table_constraints \
         WHERE table_schema = DATABASE() AND table_name = 'biz_tenant_fence' \
         ORDER BY constraint_name",
    ))
    .all(db)
    .await?;
    let expected_constraints = [
        ("PRIMARY", "PRIMARY KEY"),
        ("ck_biz_tenant_fence_generation", "CHECK"),
        ("ck_biz_tenant_fence_state", "CHECK"),
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
        return Err(schema_fingerprint_mismatch("fence constraints"));
    }

    let checks = FenceCheckRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT tc.constraint_name AS `constraint_name`, cc.check_clause AS `check_clause` \
         FROM information_schema.table_constraints tc \
         INNER JOIN information_schema.check_constraints cc \
           ON cc.constraint_schema = tc.constraint_schema \
          AND cc.constraint_name = tc.constraint_name \
         WHERE tc.table_schema = DATABASE() AND tc.table_name = 'biz_tenant_fence' \
           AND tc.constraint_type = 'CHECK'",
    ))
    .all(db)
    .await?;
    if checks.len() != 2 {
        return Err(schema_fingerprint_mismatch("fence check count"));
    }
    let generation = checks
        .iter()
        .find(|check| check.constraint_name == "ck_biz_tenant_fence_generation")
        .map(|check| normalize_check_clause(&check.check_clause));
    let state = checks
        .iter()
        .find(|check| check.constraint_name == "ck_biz_tenant_fence_state")
        .map(|check| normalize_check_clause(&check.check_clause));
    let expected_generation = normalize_check_clause("`placement_generation` > 0");
    let expected_state = normalize_check_clause("`state` IN ('active', 'frozen')");
    if generation.as_deref() != Some(expected_generation.as_str())
        || state.as_deref() != Some(expected_state.as_str())
    {
        return Err(schema_fingerprint_mismatch("fence check constraints"));
    }
    Ok(())
}
