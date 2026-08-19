pub mod engine;
pub mod naming;
pub mod schema;
pub mod template;
pub mod type_mapping;

/// 生成器版本号；生成边界或端口签名变化时递增。
pub const GENERATOR_VERSION: &str = "0.9.0";

pub use engine::{
    GenerateOptions, GeneratedFile, WriteReport, generate, normalize_relative_output_path,
    render_tables, validate_table_name, write_to_disk,
};
pub use schema::{ColumnInfo, ForeignKeyInfo, IndexInfo, TableInfo, fetch_table, list_tables};
