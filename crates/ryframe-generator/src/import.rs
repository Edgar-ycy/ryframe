//! 既有表导入资源草案的纯模型，以及按需启用的 MySQL 结构读取器。

mod draft;
mod model;

#[cfg(feature = "schema-import")]
mod mysql;
#[cfg(feature = "schema-import")]
mod type_mapping;

pub use draft::{ResourceDraft, ResourceDraftMetadata, draft_resource_from_table};
pub use model::{ColumnInfo, ForeignKeyInfo, IndexInfo, TableInfo, rust_model_source};

#[cfg(feature = "schema-import")]
pub use mysql::{inspect_existing_table, list_existing_tables};
