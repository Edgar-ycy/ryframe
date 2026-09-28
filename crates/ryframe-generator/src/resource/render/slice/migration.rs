use super::{ResourceIr, StorageKind, sql_literal};

pub(super) fn migration(resource: &ResourceIr, header: &str) -> String {
    assert!(
        resource.bootstrap_migration,
        "只有显式声明的资源可以生成首次引入迁移"
    );
    let migration_name = migration_name(resource);
    let mut definitions = resource
        .fields
        .iter()
        .map(column_definition)
        .collect::<Vec<_>>();
    definitions.push(format!(
        "  PRIMARY KEY ({})",
        resource
            .primary_key
            .iter()
            .map(|field| format!("`{}`", resource.column(field)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for index in &resource.indexes {
        definitions.push(format!(
            "  {}KEY `{}` ({})",
            if index.unique { "UNIQUE " } else { "" },
            index.name,
            index
                .fields
                .iter()
                .map(|field| format!("`{}`", resource.column(field)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if resource.storage == StorageKind::ControlRow {
        definitions.push(format!(
            "  CONSTRAINT `fk_{}_tenant` FOREIGN KEY (`{}`) REFERENCES `sys_tenant` (`tenant_id`)",
            resource.name,
            resource.column("tenant_id"),
        ));
    }
    let ddl = format!(
        "CREATE TABLE `{}` (\n{}\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='{}'",
        resource.table,
        definitions.join(",\n"),
        resource.labels.zh_cn.replace('\'', "''"),
    );
    format!(
        r##"{header}// schema-sha256: {schema_hash}

use sea_orm::{{ConnectionTrait, DbBackend}};
use sea_orm_migration::prelude::*;

pub const MIGRATION_NAME: &str = {migration_name:?};
pub const CREATE_TABLE_DDL: &str = r#"{ddl}"#;

/// 资源初始迁移；正式发布前可显式重整基线，正式发布后只能追加向前迁移。
pub const INITIAL_RESOURCE_MIGRATION: bool = true;

pub struct Migration;

impl MigrationName for Migration {{
    fn name(&self) -> &str {{
        MIGRATION_NAME
    }}
}}

#[async_trait::async_trait]
impl MigrationTrait for Migration {{
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {{
        if manager.get_database_backend() != DbBackend::MySql {{
            return Err(DbErr::Custom("资源迁移只支持 MySQL 8".into()));
        }}
        manager
            .get_connection()
            .execute_unprepared(CREATE_TABLE_DDL)
            .await?;
        Ok(())
    }}

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {{
        Err(DbErr::Custom(
            "资源迁移只允许追加和向前修复，禁止生产 down".into(),
        ))
    }}
}}
"##,
        schema_hash = resource.schema_hash,
    )
}

pub(super) fn migration_name(resource: &ResourceIr) -> String {
    format!("m_resource_initial_{}", resource.name)
}

fn column_definition(field: &super::FieldIr) -> String {
    let sql_type = super::super::schema::column_type(field);
    let nullability = if field.nullable { "NULL" } else { "NOT NULL" };
    let default = field
        .default
        .as_ref()
        .map(|value| format!(" DEFAULT {}", sql_literal(value)))
        .unwrap_or_default();
    format!("  `{}` {sql_type} {nullability}{default}", field.column)
}
