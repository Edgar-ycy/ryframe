//! 租户业务数据目标的独立 MySQL schema 迁移。
//!
//! 本 crate 只管理可放置到 shared-control 或 dedicated MySQL 的租户业务数据表，
//! 使用独立账本 `seaql_tenant_data_migrations`。控制面 `sys_*` 表只能由
//! `ryframe-db-migration` 管理，不能经此入口安装到 dedicated 目标。

use std::fmt::Write as _;

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbBackend, DbErr, FromQueryResult,
    Statement, TransactionTrait, TryGetable,
};
use sea_orm_migration::prelude::*;

mod catalog;
mod generated_catalog;
mod m20260817_000001_tenant_fence;
mod m20260817_000002_reconcile_shared_control_fence;
mod m20260817_000003_target_slot;

pub use catalog::{
    TENANT_DATA_CATALOG, TENANT_DATA_SCHEMA_FINGERPRINT, TenantDataCatalog,
    TenantDataForeignKeyDescriptor, TenantDataTableDescriptor, catalog_entry_canonical,
    schema_fingerprint_for_catalog,
};

pub const TENANT_DATA_MIGRATION_LEDGER: &str = "seaql_tenant_data_migrations";
const MIGRATION_LOCK_SQL_PREFIX: &str = "ryframe:tenant-data-migration:";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationStatus {
    pub applied: usize,
    pub expected: usize,
    pub schema_fingerprint: &'static str,
}

impl MigrationStatus {
    pub fn is_up_to_date(&self) -> bool {
        self.applied == self.expected
    }
}

pub struct Migrator;

#[derive(Debug, FromQueryResult)]
struct MigrationVersionRow {
    version: String,
}

#[derive(Debug, FromQueryResult)]
struct TableNameRow {
    table_name: String,
}

#[derive(Debug, FromQueryResult)]
struct FenceColumnRow {
    column_name: String,
    column_type: String,
    is_nullable: String,
    character_set_name: Option<String>,
    collation_name: Option<String>,
    column_key: String,
    column_default: Option<String>,
    extra: String,
    generation_expression: String,
}

#[derive(Debug, FromQueryResult)]
struct FenceIndexRow {
    index_name: String,
    column_name: String,
    seq_in_index: i64,
    non_unique: i64,
    index_type: String,
    sub_part: Option<i64>,
    is_visible: String,
}

#[derive(Debug, FromQueryResult)]
struct FenceCheckRow {
    constraint_name: String,
    check_clause: String,
}

#[derive(Debug, FromQueryResult)]
struct TenantDataTableRow {
    table_name: String,
    engine: String,
    character_set_name: String,
    table_collation: String,
}

#[derive(Debug, FromQueryResult)]
struct FenceConstraintRow {
    constraint_name: String,
    constraint_type: String,
    enforced: String,
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

#[derive(Debug, FromQueryResult)]
struct ServerIdentityRow {
    version: String,
    version_comment: String,
}

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260817_000001_tenant_fence::Migration),
            Box::new(m20260817_000002_reconcile_shared_control_fence::Migration),
            Box::new(m20260817_000003_target_slot::Migration),
        ]
    }

    fn migration_table_name() -> DynIden {
        Alias::new(TENANT_DATA_MIGRATION_LEDGER).into_iden()
    }
}

/// 升级一个明确选择的租户数据目标。不会创建任何控制面 `sys_*` 表。
pub async fn up(db: &DatabaseConnection) -> Result<(), DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
    TENANT_DATA_CATALOG
        .validate()
        .map_err(|error| DbErr::Custom(format!("tenant-data catalog is invalid: {error}")))?;

    let transaction = db.begin().await?;
    if let Err(error) = acquire_migration_lock(&transaction).await {
        let _ = transaction.rollback().await;
        return Err(error);
    }
    let migration_result = Migrator::up(&transaction, None)
        .await
        .map_err(|error| DbErr::Custom(format!("tenant-data migration failed: {error}")));
    let release_result = release_migration_lock(&transaction).await;
    match migration_result.and(release_result) {
        Ok(()) => transaction.commit().await?,
        Err(error) => {
            let _ = transaction.rollback().await;
            return Err(error);
        }
    }
    verify(db).await
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
    let status = status(db).await?;
    if !status.is_up_to_date() {
        return Err(DbErr::Custom(format!(
            "tenant-data migration ledger is not current: applied {}, expected {}; run `ryframe-migrate tenant-data up`",
            status.applied, status.expected
        )));
    }
    verify_migration_versions(db).await?;
    verify_fence_schema(db, catalog).await
}

/// 读取独立迁移账本，不执行 DDL。
pub async fn status(db: &DatabaseConnection) -> Result<MigrationStatus, DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
    let expected = Migrator::migrations().len();
    let ledger_exists = scalar_i64(
        db,
        "SELECT CAST(COUNT(*) AS SIGNED) AS `table_count` FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = 'seaql_tenant_data_migrations'",
    )
    .await?
        > 0;
    let applied = if ledger_exists {
        scalar_i64(
            db,
            "SELECT CAST(COUNT(*) AS SIGNED) FROM seaql_tenant_data_migrations",
        )
        .await? as usize
    } else {
        0
    };
    Ok(MigrationStatus {
        applied,
        expected,
        schema_fingerprint: TENANT_DATA_SCHEMA_FINGERPRINT,
    })
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
    let supported = supports_mysql_80_or_newer(&identity.version, &identity.version_comment);
    if !supported {
        return Err(DbErr::Custom(
            "tenant-data target requires MySQL 8.0.16 or newer".into(),
        ));
    }
    Ok(())
}

fn supports_mysql_80_or_newer(version: &str, version_comment: &str) -> bool {
    if version.to_ascii_lowercase().contains("mariadb")
        || version_comment.to_ascii_lowercase().contains("mariadb")
    {
        return false;
    }

    let version_core = version.split(['-', '+']).next().unwrap_or_default();
    let mut parts = version_core.split('.');
    let Some(major) = parts.next().and_then(|part| part.parse::<u32>().ok()) else {
        return false;
    };
    let Some(minor) = parts.next().and_then(|part| part.parse::<u32>().ok()) else {
        return false;
    };
    let Some(patch) = parts.next().and_then(|part| part.parse::<u32>().ok()) else {
        return false;
    };

    (major, minor, patch) >= (8, 0, 16)
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

fn ensure_mysql(db: &DatabaseConnection) -> Result<(), DbErr> {
    if db.get_database_backend() != DatabaseBackend::MySql {
        return Err(DbErr::Custom(
            "RyFrame tenant-data migrations support MySQL only".into(),
        ));
    }
    Ok(())
}

/// 从 MySQL information_schema 读取单张 catalog 表的完整、稳定结构描述。
/// Generator 写入 descriptor 与运行时 verify 共用此实现，避免规范化漂移。
pub async fn canonical_table_schema(
    db: &DatabaseConnection,
    table_name: &str,
) -> Result<String, DbErr> {
    ensure_mysql(db)?;
    if !table_name.starts_with("biz_")
        || !table_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(DbErr::Custom(
            "invalid tenant-data catalog table name".into(),
        ));
    }
    let table = TenantDataTableRow::find_by_statement(Statement::from_sql_and_values(
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
    .ok_or_else(|| DbErr::Custom(format!("tenant-data catalog table missing: {table_name}")))?;

    let columns = FenceColumnRow::find_by_statement(Statement::from_sql_and_values(
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
    let indexes = FenceIndexRow::find_by_statement(Statement::from_sql_and_values(
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
    let constraints = FenceConstraintRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT constraint_name AS `constraint_name`, constraint_type AS `constraint_type`, \
                enforced AS `enforced` \
         FROM information_schema.table_constraints WHERE table_schema = DATABASE() \
           AND table_name = ? ORDER BY constraint_name",
        [table_name.into()],
    ))
    .all(db)
    .await?;
    let checks = FenceCheckRow::find_by_statement(Statement::from_sql_and_values(
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
        .expect("writing canonical schema to String cannot fail");
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
        .expect("writing canonical schema to String cannot fail");
    }
    canonical.push_str("]|constraints=[");
    for constraint in constraints {
        write!(
            canonical,
            "{:?}:{:?}:{:?};",
            constraint.constraint_name, constraint.constraint_type, constraint.enforced,
        )
        .expect("writing canonical schema to String cannot fail");
    }
    canonical.push_str("]|checks=[");
    for check in checks {
        write!(
            canonical,
            "{:?}:{:?};",
            check.constraint_name,
            normalize_check_clause(&check.check_clause),
        )
        .expect("writing canonical schema to String cannot fail");
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
        .expect("writing canonical schema to String cannot fail");
    }
    canonical.push(']');
    Ok(canonical)
}

fn ensure_local_foreign_key_schemas(foreign_keys: &[ForeignKeySchemaRow]) -> Result<(), DbErr> {
    if foreign_keys
        .iter()
        .any(|foreign_key| foreign_key.referenced_table_schema != foreign_key.current_schema)
    {
        return Err(DbErr::Custom(
            "tenant-data catalog foreign keys must stay within the target schema".into(),
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
    for descriptor in catalog.tables() {
        let actual = canonical_table_schema(db, descriptor.table).await?;
        if actual != descriptor.schema_canonical {
            return Err(schema_fingerprint_mismatch("catalog table structure"));
        }
    }
    Ok(())
}

async fn verify_target_slot_schema(
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
    if checks.len() != 2
        || slot_id.as_deref() != Some("slot_id=1")
        || value.as_deref()
            != Some(
                "((tenant_idisnull)and(placement_generationisnull)and(switch_tokenisnull))or(\
                 (tenant_idisnotnull)and(placement_generation>0)and(switch_tokenisnotnull))",
            )
    {
        return Err(schema_fingerprint_mismatch("target slot check constraints"));
    }
    Ok(())
}

async fn verify_migration_versions(db: &DatabaseConnection) -> Result<(), DbErr> {
    let mut expected = Migrator::migrations()
        .into_iter()
        .map(|migration| migration.name().to_owned())
        .collect::<Vec<_>>();
    expected.sort_unstable();
    let actual = MigrationVersionRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT version FROM seaql_tenant_data_migrations ORDER BY version",
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|migration| migration.version)
    .collect::<Vec<_>>();
    if actual != expected {
        return Err(DbErr::Custom(
            "tenant-data migration ledger versions do not match this application build".into(),
        ));
    }
    Ok(())
}

fn normalize_check_clause(value: &str) -> String {
    // MySQL information_schema may render literal delimiters as
    // `_utf8mb4\'value\'`. Only normalize syntax outside literals. Literal
    // bytes (including case, whitespace, backticks and charset-like text) are
    // semantic and must survive the exact comparison unchanged.
    let bytes = value.as_bytes();
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if let Some(introducer_len) = charset_introducer_len(bytes, index) {
            index += introducer_len;
            continue;
        }
        if let Some((delimiter, consumed)) = quote_token_at(bytes, index) {
            normalized.push(delimiter);
            index += consumed;
            index =
                normalize_quoted_literal(bytes, index, delimiter, consumed == 2, &mut normalized);
            continue;
        }
        let byte = bytes[index];
        if !byte.is_ascii_whitespace() && byte != b'`' {
            normalized.push(byte.to_ascii_lowercase());
        }
        index += 1;
    }
    strip_redundant_outer_parentheses(
        String::from_utf8(normalized).expect("CHECK clause normalization preserves UTF-8"),
    )
}

fn charset_introducer_len(bytes: &[u8], index: usize) -> Option<usize> {
    [b"_utf8mb4".as_slice(), b"_ascii".as_slice()]
        .into_iter()
        .find(|introducer| {
            let end = index + introducer.len();
            bytes
                .get(index..end)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(introducer))
                && quote_token_at(bytes, end).is_some()
        })
        .map(<[u8]>::len)
}

fn quote_token_at(bytes: &[u8], index: usize) -> Option<(u8, usize)> {
    match bytes.get(index).copied() {
        Some(delimiter @ (b'\'' | b'"')) => Some((delimiter, 1)),
        Some(b'\\') => bytes
            .get(index + 1)
            .copied()
            .filter(|byte| matches!(byte, b'\'' | b'"'))
            .map(|delimiter| (delimiter, 2)),
        _ => None,
    }
}

fn normalize_quoted_literal(
    bytes: &[u8],
    mut index: usize,
    delimiter: u8,
    escaped_delimiters: bool,
    output: &mut Vec<u8>,
) -> usize {
    while index < bytes.len() {
        if escaped_delimiters && bytes.get(index) == Some(&b'\\') {
            let slash_start = index;
            while bytes.get(index) == Some(&b'\\') {
                index += 1;
            }
            let slash_count = index - slash_start;
            if bytes.get(index) == Some(&delimiter) {
                if slash_count == 1 {
                    output.push(delimiter);
                    return index + 1;
                }
                if slash_count >= 3 && slash_count % 2 == 1 {
                    // MySQL may render an apostrophe inside an escaped-delimiter
                    // literal as three slashes plus the quote. Canonicalize it to
                    // SQL's doubled-quote representation while preserving any
                    // additional literal backslashes.
                    output.extend(std::iter::repeat_n(b'\\', (slash_count - 3) / 2));
                    output.extend_from_slice(&[delimiter, delimiter]);
                    index += 1;
                    continue;
                }
            }
            output.extend_from_slice(&bytes[slash_start..index]);
            continue;
        }
        if !escaped_delimiters
            && bytes.get(index) == Some(&b'\\')
            && bytes.get(index + 1) == Some(&delimiter)
        {
            output.extend_from_slice(&[delimiter, delimiter]);
            index += 2;
            continue;
        }
        if bytes[index] == delimiter {
            if !escaped_delimiters && bytes.get(index + 1) == Some(&delimiter) {
                output.extend_from_slice(&[delimiter, delimiter]);
                index += 2;
                continue;
            }
            output.push(delimiter);
            return index + 1;
        }
        output.push(bytes[index]);
        index += 1;
    }
    index
}

fn strip_redundant_outer_parentheses(mut value: String) -> String {
    while is_wrapped_by_single_outer_group(&value) {
        value = value[1..value.len() - 1].to_owned();
    }
    value
}

fn is_wrapped_by_single_outer_group(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'(' || bytes[bytes.len() - 1] != b')' {
        return false;
    }
    let mut depth = 0_i32;
    let mut quote = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if byte == b'\\' {
                index = (index + 2).min(bytes.len());
                continue;
            }
            if byte == delimiter {
                if bytes.get(index + 1) == Some(&delimiter) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'(' {
            depth += 1;
        } else if byte == b')' {
            depth -= 1;
            if depth == 0 && index + 1 != bytes.len() {
                return false;
            }
            if depth < 0 {
                return false;
            }
        }
        index += 1;
    }
    depth == 0 && quote.is_none()
}

fn normalize_column_default(value: Option<&str>) -> Option<&str> {
    value.map(|value| {
        if value.eq_ignore_ascii_case("current_timestamp(6)") {
            "current_timestamp(6)"
        } else {
            value
        }
    })
}

fn normalize_column_extra(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .split_whitespace()
        .filter(|part| *part != "default_generated")
        .collect::<Vec<_>>()
        .join(" ")
}

fn schema_fingerprint_mismatch(detail: &str) -> DbErr {
    DbErr::Custom(format!(
        "tenant-data schema fingerprint mismatch ({detail}): expected {TENANT_DATA_SCHEMA_FINGERPRINT}"
    ))
}

async fn scalar_i64(db: &DatabaseConnection, sql: &str) -> Result<i64, DbErr> {
    scalar_i64_statement(db, Statement::from_string(DbBackend::MySql, sql.to_owned())).await
}

async fn scalar_i64_statement(db: &DatabaseConnection, statement: Statement) -> Result<i64, DbErr> {
    let row = db
        .query_one_raw(statement)
        .await?
        .ok_or_else(|| DbErr::Custom("tenant-data verification query returned no result".into()))?;
    Option::<i64>::try_get_by_index(&row, 0)?.ok_or_else(|| {
        DbErr::Custom("tenant-data verification query returned a NULL scalar".into())
    })
}

async fn acquire_migration_lock<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let value = scalar_i64_on(
        db,
        format!(
            "SELECT GET_LOCK(SHA2(CONCAT('{MIGRATION_LOCK_SQL_PREFIX}', DATABASE()), 256), 60)"
        ),
    )
    .await?;
    if value != 1 {
        return Err(DbErr::Custom(
            "timed out waiting for the tenant-data migration lock".into(),
        ));
    }
    Ok(())
}

async fn release_migration_lock<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let value = scalar_i64_on(
        db,
        format!(
            "SELECT RELEASE_LOCK(SHA2(CONCAT('{MIGRATION_LOCK_SQL_PREFIX}', DATABASE()), 256))"
        ),
    )
    .await?;
    if value != 1 {
        return Err(DbErr::Custom(
            "failed to release the tenant-data migration lock".into(),
        ));
    }
    Ok(())
}

async fn scalar_i64_on<C>(db: &C, sql: String) -> Result<i64, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql))
        .await?
        .ok_or_else(|| DbErr::Custom("tenant-data migration lock returned no result".into()))?;
    Option::<i64>::try_get_by_index(&row, 0)?
        .ok_or_else(|| DbErr::Custom("tenant-data migration lock returned NULL".into()))
}

#[cfg(test)]
mod tests {
    use super::{ForeignKeySchemaRow, ensure_local_foreign_key_schemas, normalize_check_clause};

    #[test]
    fn normalizes_mysql_escaped_check_literal_quotes() {
        assert_eq!(
            normalize_check_clause(r#"(`state` IN (_utf8mb4\'active\', _utf8mb4\'frozen\'))"#),
            "statein('active','frozen')",
        );
        assert_eq!(
            normalize_check_clause(r#"(`kind` = _ascii\"mysql\")"#),
            "kind=\"mysql\"",
        );
        assert_eq!(
            normalize_check_clause(r#"(`code` = _utf8mb4\'O\\\'Reilly\')"#),
            normalize_check_clause("`code` = 'O''Reilly'"),
        );
        assert_ne!(
            normalize_check_clause(r#"(`code` = _utf8mb4\'O\\\'Reilly\')"#),
            normalize_check_clause("`code` = 'OReilly'"),
        );
    }

    #[test]
    fn preserves_semantic_check_grouping() {
        assert_eq!(normalize_check_clause("(((`a` = 1)))"), "a=1");
        assert_ne!(
            normalize_check_clause("((`a` AND `b`) OR `c`)"),
            normalize_check_clause("(`a` AND (`b` OR `c`))"),
        );
    }

    #[test]
    fn preserves_literal_bytes_and_charset_like_text() {
        assert_ne!(
            normalize_check_clause("`code` = 'A'"),
            normalize_check_clause("`code` = 'a'"),
        );
        assert_ne!(
            normalize_check_clause("`label` = 'a b'"),
            normalize_check_clause("`label` = 'ab'"),
        );
        assert_ne!(
            normalize_check_clause("`code` = '_utf8mb4active'"),
            normalize_check_clause("`code` = 'active'"),
        );
        assert_ne!(
            normalize_check_clause("`code` = 'A` B'"),
            normalize_check_clause("`code` = 'a b'"),
        );
    }

    #[test]
    fn rejects_cross_schema_foreign_keys() {
        let row = ForeignKeySchemaRow {
            constraint_name: "fk_child_parent".into(),
            column_name: "parent_id".into(),
            ordinal_position: 1,
            referenced_table_schema: "control".into(),
            current_schema: "tenant_data".into(),
            referenced_table_name: "biz_parent".into(),
            referenced_column_name: "id".into(),
            update_rule: "RESTRICT".into(),
            delete_rule: "RESTRICT".into(),
        };
        assert!(ensure_local_foreign_key_schemas(&[row]).is_err());
    }
}
