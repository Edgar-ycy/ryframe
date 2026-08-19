use serde::Serialize;
use utoipa::ToSchema;

use ryframe_application::system::generator_service::{
    ColumnInfo as ServiceColumnInfo, ForeignKeyInfo as ServiceForeignKeyInfo,
    GeneratedFile as ServiceGeneratedFile, IndexInfo as ServiceIndexInfo,
    TableInfo as ServiceTableInfo, WriteReport as ServiceWriteReport,
};

/// 数据库表结构响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct TableInfo {
    pub table_name: String,
    pub comment: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub foreign_key_dependencies: Vec<String>,
    pub schema_canonical: String,
}

impl From<ServiceTableInfo> for TableInfo {
    fn from(value: ServiceTableInfo) -> Self {
        let ServiceTableInfo {
            table_name,
            comment,
            columns,
            indexes,
            foreign_keys,
            foreign_key_dependencies,
            schema_canonical,
        } = value;
        Self {
            table_name,
            comment,
            columns: columns.into_iter().map(ColumnInfo::from).collect(),
            indexes: indexes.into_iter().map(IndexInfo::from).collect(),
            foreign_keys: foreign_keys.into_iter().map(ForeignKeyInfo::from).collect(),
            foreign_key_dependencies,
            schema_canonical,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub index_type: String,
    pub columns: Vec<String>,
}

impl From<ServiceIndexInfo> for IndexInfo {
    fn from(value: ServiceIndexInfo) -> Self {
        Self {
            name: value.name,
            unique: value.unique,
            index_type: value.index_type,
            columns: value.columns,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ForeignKeyInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
}

impl From<ServiceForeignKeyInfo> for ForeignKeyInfo {
    fn from(value: ServiceForeignKeyInfo) -> Self {
        Self {
            name: value.name,
            columns: value.columns,
            referenced_table: value.referenced_table,
            referenced_columns: value.referenced_columns,
        }
    }
}

/// 数据库列结构响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub rust_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub is_auto_increment: bool,
    pub comment: Option<String>,
}

impl From<ServiceColumnInfo> for ColumnInfo {
    fn from(value: ServiceColumnInfo) -> Self {
        let ServiceColumnInfo {
            name,
            data_type,
            rust_type,
            is_nullable,
            is_primary_key,
            is_unique,
            is_auto_increment,
            comment,
        } = value;
        Self {
            name,
            data_type,
            rust_type,
            is_nullable,
            is_primary_key,
            is_unique,
            is_auto_increment,
            comment,
        }
    }
}

/// 代码生成预览文件。
#[derive(Debug, Serialize, ToSchema)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

impl From<ServiceGeneratedFile> for GeneratedFile {
    fn from(value: ServiceGeneratedFile) -> Self {
        let ServiceGeneratedFile { path, content } = value;
        Self { path, content }
    }
}

/// 代码生成写入报告。
#[derive(Debug, Serialize, ToSchema)]
pub struct WriteReport {
    pub written: Vec<String>,
    pub skipped: Vec<String>,
}

impl From<ServiceWriteReport> for WriteReport {
    fn from(value: ServiceWriteReport) -> Self {
        let ServiceWriteReport { written, skipped } = value;
        Self { written, skipped }
    }
}
