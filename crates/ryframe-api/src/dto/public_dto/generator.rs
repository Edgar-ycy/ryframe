use serde::Serialize;
use utoipa::ToSchema;

use ryframe_service::system::generator_service::{
    ColumnInfo as ServiceColumnInfo, GeneratedFile as ServiceGeneratedFile,
    TableInfo as ServiceTableInfo, WriteReport as ServiceWriteReport,
};

/// 数据库表结构响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct TableInfo {
    pub table_name: String,
    pub comment: Option<String>,
    pub columns: Vec<ColumnInfo>,
}

impl From<ServiceTableInfo> for TableInfo {
    fn from(value: ServiceTableInfo) -> Self {
        let ServiceTableInfo {
            table_name,
            comment,
            columns,
        } = value;
        Self {
            table_name,
            comment,
            columns: columns.into_iter().map(ColumnInfo::from).collect(),
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
