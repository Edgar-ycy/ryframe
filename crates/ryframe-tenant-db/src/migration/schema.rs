use sea_orm::{DatabaseBackend, DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement};

use super::catalog::{TENANT_DATA_CATALOG, TENANT_DATA_SCHEMA_FINGERPRINT, TenantDataCatalog};
use super::normalization::normalize_check_clause;
use super::status::{TENANT_DATA_MIGRATION_LEDGER, status_after_server_validation};
use catalog::{
    FenceCheckRow, FenceColumnRow, FenceConstraintRow, FenceIndexRow, TenantDataTableRow,
};
use target_slot::verify_target_slot_schema;

mod catalog;
mod target_slot;

pub use catalog::{canonical_table_schema, ensure_local_foreign_key_schema};

#[derive(Debug, FromQueryResult)]
struct TableNameRow {
    table_name: String,
}

#[derive(Debug, FromQueryResult)]
struct ServerIdentityRow {
    version: String,
    version_comment: String,
}

/// 只读校验独立账本及当前租户数据 schema 契约。
pub async fn verify(db: &DatabaseConnection) -> Result<(), DbErr> {
    TENANT_DATA_CATALOG
        .validate()
        .map_err(|error| DbErr::Custom(format!("tenant-data catalog is invalid: {error}")))?;
    verify_for_catalog(db, &TENANT_DATA_CATALOG).await
}

/// 使用调用方注入的静态 catalog 做完整 schema 校验。生产入口仍由 [`verify`]
/// 额外校验生成指纹常量；此入口用于不污染产品 catalog 的集成测试。
pub async fn verify_for_catalog(
    db: &DatabaseConnection,
    catalog: &TenantDataCatalog,
) -> Result<(), DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
    catalog
        .validate_structure()
        .map_err(|error| DbErr::Custom(format!("tenant-data catalog is invalid: {error}")))?;
    let status = status_after_server_validation(db).await?;
    if !status.is_up_to_date() {
        return Err(DbErr::Custom(format!(
            "tenant-data migration ledger is not current: applied {}, expected {}, missing [{}], unexpected [{}]; run `ryframe-migrate tenant-data up`",
            status.applied,
            status.expected,
            status.missing.join(","),
            status.unexpected.join(",")
        )));
    }
    verify_fence_schema(db, catalog).await
}

/// 统一数据面只接受 MySQL 8.0.16 或更高版本；错误契约不回显服务器原始身份。
pub async fn verify_mysql_80(db: &DatabaseConnection) -> Result<(), DbErr> {
    ensure_mysql(db)?;
    let identity = ServerIdentityRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT VERSION() AS `version`, @@version_comment AS `version_comment`",
    ))
    .one(db)
    .await?
    .ok_or_else(|| DbErr::Custom("cannot verify MySQL server identity".into()))?;
    let supported = ryframe_db::migration::supports_mysql_80_or_newer(
        &identity.version,
        &identity.version_comment,
    );
    if !supported {
        return Err(DbErr::Custom(
            "tenant-data target requires MySQL 8.0.16 or newer".into(),
        ));
    }
    Ok(())
}

/// dedicated/mysql 目标不得混入控制面对象；shared-control 使用 `verify`。
pub async fn verify_mysql_target(db: &DatabaseConnection) -> Result<(), DbErr> {
    TENANT_DATA_CATALOG
        .validate()
        .map_err(|error| DbErr::Custom(format!("tenant-data catalog is invalid: {error}")))?;
    verify_mysql_target_for_catalog(db, &TENANT_DATA_CATALOG).await
}

pub async fn verify_mysql_target_for_catalog(
    db: &DatabaseConnection,
    catalog: &TenantDataCatalog,
) -> Result<(), DbErr> {
    verify_for_catalog(db, catalog).await?;
    let actual = mysql_target_table_names(db).await?;
    let expected = expected_mysql_target_table_names(catalog);
    if actual != expected {
        return Err(DbErr::Custom(
            "mysql tenant-data target table set does not match the compiled catalog".into(),
        ));
    }
    Ok(())
}

pub async fn ensure_mysql_target_boundary(db: &DatabaseConnection) -> Result<(), DbErr> {
    verify_mysql_80(db).await?;
    let expected = expected_mysql_target_table_names(&TENANT_DATA_CATALOG);
    let actual = mysql_target_table_names(db).await?;
    if actual.iter().any(|table| !expected.contains(table)) {
        return Err(DbErr::Custom(
            "mysql tenant-data target contains objects outside the compiled tenant-data schema"
                .into(),
        ));
    }
    Ok(())
}

fn expected_mysql_target_table_names(catalog: &TenantDataCatalog) -> Vec<String> {
    let mut expected = vec![
        TENANT_DATA_MIGRATION_LEDGER.to_owned(),
        "biz_tenant_fence".to_owned(),
        "biz_tenant_target_slot".to_owned(),
        "ryframe_resource_ownership".to_owned(),
    ];
    expected.extend(
        catalog
            .tables()
            .iter()
            .map(|descriptor| descriptor.table.to_owned()),
    );
    expected.sort_unstable();
    expected
}

async fn mysql_target_table_names(db: &DatabaseConnection) -> Result<Vec<String>, DbErr> {
    let mut tables = TableNameRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT table_name AS `table_name` FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE' ORDER BY table_name",
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| row.table_name)
    .collect::<Vec<_>>();
    tables.sort_unstable();
    Ok(tables)
}

pub(super) fn ensure_mysql(db: &DatabaseConnection) -> Result<(), DbErr> {
    if db.get_database_backend() != DatabaseBackend::MySql {
        return Err(DbErr::Custom(
            "RyFrame tenant-data migrations support MySQL only".into(),
        ));
    }
    Ok(())
}

async fn verify_fence_schema(
    db: &DatabaseConnection,
    catalog: &TenantDataCatalog,
) -> Result<(), DbErr> {
    let tables = TenantDataTableRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT t.table_name AS `table_name`, t.engine AS `engine`, \
                c.character_set_name AS `character_set_name`, \
                t.table_collation AS `table_collation` \
         FROM information_schema.tables t \
         INNER JOIN information_schema.collation_character_set_applicability c \
           ON c.collation_name = t.table_collation \
         WHERE t.table_schema = DATABASE() AND t.table_type = 'BASE TABLE' \
         ORDER BY t.table_name",
    ))
    .all(db)
    .await?;
    let mut actual_business_tables = tables
        .iter()
        .filter(|table| table.table_name.starts_with("biz_"))
        .map(|table| table.table_name.as_str())
        .collect::<Vec<_>>();
    let mut expected_business_tables = vec!["biz_tenant_fence", "biz_tenant_target_slot"];
    expected_business_tables.extend(catalog.tables().iter().map(|table| table.table));
    actual_business_tables.sort_unstable();
    expected_business_tables.sort_unstable();
    if actual_business_tables != expected_business_tables {
        return Err(schema_fingerprint_mismatch(
            "tenant-data table set (unknown or missing biz_ table)",
        ));
    }
    let fence = tables
        .iter()
        .find(|table| table.table_name == "biz_tenant_fence")
        .ok_or_else(|| schema_fingerprint_mismatch("fence table"))?;
    if !fence.engine.eq_ignore_ascii_case("InnoDB")
        || !fence.character_set_name.eq_ignore_ascii_case("utf8mb4")
        || !fence
            .table_collation
            .eq_ignore_ascii_case("utf8mb4_general_ci")
    {
        return Err(schema_fingerprint_mismatch(
            "fence engine/character-set/collation",
        ));
    }

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
    if generation.as_deref() != Some("placement_generation>0")
        || state.as_deref() != Some("statein('active','frozen')")
    {
        return Err(schema_fingerprint_mismatch("fence check constraints"));
    }
    verify_target_slot_schema(db, &tables).await?;
    verify_resource_ownership_schema(db).await?;
    for descriptor in catalog.tables() {
        let actual = canonical_table_schema(db, descriptor.table).await?;
        if actual != descriptor.schema_canonical {
            return Err(schema_fingerprint_mismatch("catalog table structure"));
        }
    }
    Ok(())
}

async fn verify_resource_ownership_schema(db: &DatabaseConnection) -> Result<(), DbErr> {
    let actual = canonical_table_schema(db, "ryframe_resource_ownership").await?;
    if actual != super::baseline_contract::RESOURCE_OWNERSHIP_SCHEMA_DESCRIPTOR {
        return Err(schema_fingerprint_mismatch("resource ownership marker"));
    }
    Ok(())
}

pub(super) fn normalize_column_default(value: Option<&str>) -> Option<&str> {
    value.map(|value| {
        if value.eq_ignore_ascii_case("current_timestamp(6)") {
            "current_timestamp(6)"
        } else {
            value
        }
    })
}

pub(super) fn normalize_column_extra(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .split_whitespace()
        .filter(|part| *part != "default_generated")
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn schema_fingerprint_mismatch(detail: &str) -> DbErr {
    DbErr::Custom(format!(
        "tenant-data schema fingerprint mismatch ({detail}): expected {TENANT_DATA_SCHEMA_FINGERPRINT}"
    ))
}
