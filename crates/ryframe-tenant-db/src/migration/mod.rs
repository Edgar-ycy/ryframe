//! 租户业务数据目标的独立 MySQL schema 迁移。
//!
//! 本模块只管理可放置到 shared-control 或 dedicated MySQL 的租户业务数据表，
//! 使用独立账本 `seaql_tenant_data_migrations`。控制面 `sys_*` 表只能由
//! `ryframe-db::migration` 管理，不能经此入口安装到 dedicated 目标。

mod catalog;
mod generated_catalog;
mod m20260820_000000_tenant_baseline;
mod normalization;
mod runtime;
mod schema;

pub use catalog::{
    TENANT_DATA_CATALOG, TENANT_DATA_SCHEMA_FINGERPRINT, TenantDataCatalog,
    TenantDataForeignKeyDescriptor, TenantDataTableDescriptor, catalog_entry_canonical,
    schema_fingerprint_for_catalog,
};
pub use m20260820_000000_tenant_baseline::{
    RESOURCE_OWNERSHIP_DDL, TENANT_FENCE_DDL, TENANT_TARGET_SLOT_DDL,
};
pub use normalization::normalize_check_clause;
pub use runtime::{MigrationStatus, Migrator, TENANT_DATA_MIGRATION_LEDGER, status, up};
pub use schema::{
    canonical_table_schema, ensure_local_foreign_key_schema, ensure_mysql_target_boundary, verify,
    verify_for_catalog, verify_mysql_80, verify_mysql_target, verify_mysql_target_for_catalog,
};
