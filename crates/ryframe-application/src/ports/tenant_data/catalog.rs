//! 业务模块提供的租户数据复制、校验及依赖描述，不包含具体数据库类型。

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct TenantDataTableDescriptor {
    pub table: &'static str,
    pub copy_order: u32,
    pub tenant_column: &'static str,
    pub primary_key_cursor_columns: &'static [&'static str],
    pub checksum_columns: &'static [&'static str],
    pub column_types: &'static [&'static str],
    pub has_generated_columns: bool,
    pub foreign_key_dependencies: &'static [&'static str],
    pub foreign_keys: &'static [TenantDataForeignKeyDescriptor],
    pub schema_canonical: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct TenantDataForeignKeyDescriptor {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    pub referenced_table: &'static str,
    pub referenced_columns: &'static [&'static str],
}
