use super::catalog::{
    TenantDataCatalog, data_scope_dept_schema_canonical, data_scope_owner_schema_canonical,
    data_scope_projection_schema_canonical, tenant_data_schema_fingerprint,
};
use super::status::{TENANT_DATA_MIGRATION_LEDGER, status_after_server_validation};
use super::tenant_data_catalog;
use ryframe_db::migration::normalize_check_clause;
use ryframe_db::schema::{
    CheckMetadata, ColumnMetadata, ConstraintMetadata, IndexMetadata, TableMetadata,
};
use sea_orm::{DatabaseBackend, DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement};
use target_slot::verify_target_slot_schema;

mod fence;
mod target_slot;
use fence::{verify_fence_columns, verify_fence_constraints, verify_fence_indexes};

use ryframe_db::schema::{normalize_column_default, normalize_column_extra};

pub async fn canonical_table_schema(
    db: &DatabaseConnection,
    table_name: &str,
) -> Result<String, DbErr> {
    if !(table_name.starts_with("biz_") || table_name == "ryframe_resource_ownership") {
        return Err(DbErr::Custom(
            "租户数据目录只允许 biz_ 业务表与归属标记表".into(),
        ));
    }
    ryframe_db::schema::canonical_table_schema(db, table_name).await
}

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
    tenant_data_catalog()
        .validate()
        .map_err(|error| DbErr::Custom(format!("tenant-data catalog is invalid: {error}")))?;
    verify_for_catalog(db, tenant_data_catalog()).await
}

/// 使用调用方注入的静态 catalog 做完整 schema 校验。生产入口仍由 [`verify`]
/// 额外校验生成指纹常量；此入口用于不污染产品 catalog 的集成测试。
pub async fn verify_for_catalog(
    db: &DatabaseConnection,
    catalog: &TenantDataCatalog,
) -> Result<(), DbErr> {
    verify_scoped(db, catalog, &std::collections::BTreeSet::new()).await
}

/// 仅显式 shared-control 调用方可排除组合根登记的控制库业务表。
pub async fn verify_shared_control(db: &DatabaseConnection) -> Result<(), DbErr> {
    tenant_data_catalog().validate().map_err(DbErr::Custom)?;
    verify_shared_control_for_catalog(db, tenant_data_catalog()).await
}

/// 显式共享目标使用注入目录校验；控制库排除集合仍仅取组合根正式登记。
pub async fn verify_shared_control_for_catalog(
    db: &DatabaseConnection,
    catalog: &TenantDataCatalog,
) -> Result<(), DbErr> {
    verify_scoped(
        db,
        catalog,
        super::business_catalog::shared_control_tables(),
    )
    .await
}

async fn verify_scoped(
    db: &DatabaseConnection,
    catalog: &TenantDataCatalog,
    control_tables: &std::collections::BTreeSet<String>,
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
    verify_fence_schema(db, catalog, control_tables).await
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

/// dedicated/mysql 目标不得混入控制面对象；shared-control 使用 `verify_shared_control`。
pub async fn verify_mysql_target(db: &DatabaseConnection) -> Result<(), DbErr> {
    tenant_data_catalog()
        .validate()
        .map_err(|error| DbErr::Custom(format!("tenant-data catalog is invalid: {error}")))?;
    verify_mysql_target_for_catalog(db, tenant_data_catalog()).await
}

pub async fn verify_mysql_target_for_catalog(
    db: &DatabaseConnection,
    catalog: &TenantDataCatalog,
) -> Result<(), DbErr> {
    verify_for_catalog(db, catalog).await?;
    let actual = mysql_target_table_names(db).await?;
    let expected = expected_mysql_target_table_names(catalog);
    if actual != expected {
        return Err(table_set_difference(
            &actual.iter().map(String::as_str).collect::<Vec<_>>(),
            &expected.iter().map(String::as_str).collect::<Vec<_>>(),
        ));
    }
    Ok(())
}

pub async fn ensure_mysql_target_boundary(db: &DatabaseConnection) -> Result<(), DbErr> {
    verify_mysql_80(db).await?;
    let expected = expected_mysql_target_table_names(tenant_data_catalog());
    let actual = mysql_target_table_names(db).await?;
    if actual.iter().any(|table| !expected.contains(table)) {
        return Err(table_set_difference(
            &actual.iter().map(String::as_str).collect::<Vec<_>>(),
            &actual
                .iter()
                .filter(|table| expected.contains(table))
                .map(String::as_str)
                .collect::<Vec<_>>(),
        ));
    }
    Ok(())
}

fn expected_mysql_target_table_names(catalog: &TenantDataCatalog) -> Vec<String> {
    let mut expected = vec![
        TENANT_DATA_MIGRATION_LEDGER.to_owned(),
        "biz_tenant_fence".to_owned(),
        "biz_tenant_target_slot".to_owned(),
        "biz_data_scope_projection".to_owned(),
        "biz_data_scope_owner".to_owned(),
        "biz_data_scope_dept".to_owned(),
        "ryframe_resource_ownership".to_owned(),
    ];
    expected.extend(
        catalog
            .tables()
            .iter()
            .map(|descriptor| descriptor.table.to_owned()),
    );
    if catalog.tables().len() > super::TENANT_DATA_CATALOG.tables().len() {
        expected.push("ryframe_business_migration".to_owned());
    }
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
    control_tables: &std::collections::BTreeSet<String>,
) -> Result<(), DbErr> {
    let tables = TableMetadata::find_by_statement(Statement::from_string(
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
        .filter(|table| {
            table.table_name.starts_with("biz_") && !control_tables.contains(&table.table_name)
        })
        .map(|table| table.table_name.as_str())
        .collect::<Vec<_>>();
    let mut expected_business_tables = vec![
        "biz_tenant_fence",
        "biz_tenant_target_slot",
        "biz_data_scope_projection",
        "biz_data_scope_owner",
        "biz_data_scope_dept",
    ];
    expected_business_tables.extend(catalog.tables().iter().map(|table| table.table));
    actual_business_tables.sort_unstable();
    expected_business_tables.sort_unstable();
    if actual_business_tables != expected_business_tables {
        return Err(table_set_difference(
            &actual_business_tables,
            &expected_business_tables,
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

    verify_fence_columns(db).await?;
    verify_fence_indexes(db).await?;
    verify_fence_constraints(db).await?;
    verify_target_slot_schema(db, &tables).await?;
    verify_resource_ownership_schema(db).await?;
    verify_data_scope_schema(db).await?;
    for descriptor in catalog.tables() {
        let actual = canonical_table_schema(db, descriptor.table).await?;
        if actual != descriptor.schema_canonical {
            return Err(schema_fingerprint_mismatch("catalog table structure"));
        }
    }
    Ok(())
}

async fn verify_data_scope_schema(db: &DatabaseConnection) -> Result<(), DbErr> {
    for (table, expected) in [
        (
            "biz_data_scope_projection",
            data_scope_projection_schema_canonical(),
        ),
        ("biz_data_scope_owner", data_scope_owner_schema_canonical()),
        ("biz_data_scope_dept", data_scope_dept_schema_canonical()),
    ] {
        let actual = canonical_table_schema(db, table).await?;
        if actual != expected {
            return Err(DbErr::Custom(format!(
                "tenant-data data scope schema mismatch for {table}: expected {expected}, actual {actual}"
            )));
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

pub(super) fn schema_fingerprint_mismatch(detail: &str) -> DbErr {
    DbErr::Custom(format!(
        "tenant-data schema fingerprint mismatch ({detail}): expected {}",
        tenant_data_schema_fingerprint()
    ))
}

fn table_set_difference(actual: &[&str], expected: &[&str]) -> DbErr {
    let unexpected = actual
        .iter()
        .filter(|table| !expected.contains(table))
        .copied()
        .collect::<Vec<_>>();
    let missing = expected
        .iter()
        .filter(|table| !actual.contains(table))
        .copied()
        .collect::<Vec<_>>();
    DbErr::Custom(format!(
        "租户表集合不一致：未知表 [{}]，缺失表 [{}]；请核对目标配置、组合根登记的 control/tenant 业务归属及迁移账本，并运行 control verify 与 tenant-data verify；当前校验只读；此前迁移可能已完成 DDL，请保留现场，勿删库或自动重放迁移",
        unexpected.join(", "),
        missing.join(", ")
    ))
}
