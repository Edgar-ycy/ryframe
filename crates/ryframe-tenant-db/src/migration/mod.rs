//! 租户业务数据目标的独立 MySQL schema 迁移。
//!
//! 本模块只管理可放置到 shared-control 或 dedicated MySQL 的租户业务数据表，
//! 使用独立账本 `seaql_tenant_data_migrations`。控制面 `sys_*` 表只能由
//! `ryframe-db-migration` 管理，不能经此入口安装到 dedicated 目标。

mod catalog;
mod generated_catalog;
mod m20260817_000001_tenant_fence;
mod m20260817_000002_reconcile_shared_control_fence;
mod m20260817_000003_target_slot;
mod normalization;
mod runtime;
mod schema;

pub use catalog::{
    TENANT_DATA_CATALOG, TENANT_DATA_SCHEMA_FINGERPRINT, TenantDataCatalog,
    TenantDataForeignKeyDescriptor, TenantDataTableDescriptor, catalog_entry_canonical,
    schema_fingerprint_for_catalog,
};
pub use runtime::{MigrationStatus, Migrator, TENANT_DATA_MIGRATION_LEDGER, status, up};
pub use schema::{
    canonical_table_schema, ensure_mysql_target_boundary, verify, verify_for_catalog,
    verify_mysql_80, verify_mysql_target, verify_mysql_target_for_catalog,
};
