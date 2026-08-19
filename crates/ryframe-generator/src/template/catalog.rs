use crate::schema::TableInfo;

pub fn render_catalog(tables: &[&TableInfo]) -> String {
    let mut canonical_entries = Vec::with_capacity(tables.len());
    let descriptors = tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let primary_key_columns = table
                .indexes
                .iter()
                .find(|index| index.name == "PRIMARY")
                .expect("table schema is validated before rendering")
                .columns
                .clone();
            let primary_key_literals = primary_key_columns
                .iter()
                .map(|column| format!("\"{column}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let checksum_columns = table
                .columns
                .iter()
                .map(|column| format!("\"{}\"", column.name))
                .collect::<Vec<_>>()
                .join(", ");
            let column_types = table
                .columns
                .iter()
                .map(|column| format!("\"{}\"", column.data_type))
                .collect::<Vec<_>>()
                .join(", ");
            let dependencies = table
                .foreign_key_dependencies
                .iter()
                .map(|dependency| format!("\"{dependency}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let foreign_keys = table
                .foreign_keys
                .iter()
                .map(|foreign_key| {
                    let columns = foreign_key
                        .columns
                        .iter()
                        .map(|column| format!("\"{column}\""))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let referenced_columns = foreign_key
                        .referenced_columns
                        .iter()
                        .map(|column| format!("\"{column}\""))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "crate::TenantDataForeignKeyDescriptor {{ name: \"{}\", columns: &[{}], referenced_table: \"{}\", referenced_columns: &[{}] }}",
                        foreign_key.name, columns, foreign_key.referenced_table, referenced_columns
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let checksum_column_names = table
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect::<Vec<_>>();
            canonical_entries.push(ryframe_tenant_db::migration::catalog_entry_canonical(
                &table.table_name,
                ((index + 1) * 10) as u32,
                "tenant_id",
                &primary_key_columns,
                &checksum_column_names,
                &table.foreign_key_dependencies,
                &table.schema_canonical,
            ));
            format!(
                r#"    TenantDataTableDescriptor {{
        table: "{table}",
        copy_order: {copy_order},
        tenant_column: "tenant_id",
        primary_key_cursor_columns: &[{primary_key_literals}],
        checksum_columns: &[{checksum_columns}],
        column_types: &[{column_types}],
        has_generated_columns: false,
        foreign_key_dependencies: &[{dependencies}],
        foreign_keys: &[{foreign_keys}],
        schema_canonical: {schema_canonical:?},
    }},"#,
                table = table.table_name,
                copy_order = (index + 1) * 10,
                primary_key_literals = primary_key_literals,
                column_types = column_types,
                schema_canonical = table.schema_canonical,
                foreign_keys = foreign_keys,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let fingerprint =
        ryframe_tenant_db::migration::schema_fingerprint_for_catalog(&canonical_entries);
    format!(
        r#"// 此文件由 ryframe-generator v{version} 自动生成。
// 将该切片合并到 ryframe-tenant-db 的编译期 TenantDataCatalog，
// 并随 tenant-data migration 一起提交；未注册的业务表不得上线。
use super::catalog::TenantDataTableDescriptor;

pub const GENERATED_TENANT_DATA_TABLES: &[TenantDataTableDescriptor] = &[
{descriptors}
];

pub const GENERATED_TENANT_DATA_SCHEMA_FINGERPRINT: &str = "{fingerprint}";
"#,
        version = crate::GENERATOR_VERSION,
    )
}
