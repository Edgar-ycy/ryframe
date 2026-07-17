pub mod engine;
pub mod naming;
pub mod schema;
pub mod template;
pub mod type_mapping;

/// 生成器版本号 — 当核心 trait 签名变更时递增此版本
///
/// 依赖的核心 traits:
/// - ryframe_core::repository::Repository
/// - ryframe_core::auto_fill::AutoFill
/// - ryframe_common::ApiResponse / ApiPageResponse
pub const GENERATOR_VERSION: &str = "0.7.0";

pub use engine::{GenerateOptions, GeneratedFile, WriteReport, generate, write_to_disk};
pub use schema::{ColumnInfo, TableInfo, fetch_table, list_tables};
