pub mod engine;
pub mod naming;
pub mod schema;
pub mod template;
pub mod type_mapping;

/// 生成器版本号 — 当核心 trait 签名变更时递增此版本
///
/// 依赖的核心 traits:
/// - 仓储 trait：ryframe_adapters::repository::Repository
/// - 自动填充 trait：ryframe_adapters::auto_fill::AutoFill
/// - HTTP 响应类型：ryframe_http::ApiResponse / ApiPageResponse
pub const GENERATOR_VERSION: &str = "0.8.0";

pub use engine::{
    GenerateOptions, GeneratedFile, WriteReport, generate, normalize_relative_output_path,
    validate_table_name, write_to_disk,
};
pub use schema::{ColumnInfo, ForeignKeyInfo, IndexInfo, TableInfo, fetch_table, list_tables};
