//! 使用者业务 crate 的发现、描述符读取和生成入口。

mod bootstrap;
mod catalog;
mod format;
mod frontend;
mod generate;
mod metadata;
mod migrations;
mod model;
pub(crate) mod writer;
pub(crate) use bootstrap::validate_module;

pub use bootstrap::{
    BusinessBootstrapOptions, BusinessBootstrapReport, bootstrap_business_package,
};
pub(crate) use frontend::generate_business_frontend;
pub use generate::{BusinessGenerateOptions, BusinessGenerateReport, generate_business_package};
pub use metadata::{BusinessPackage, locate_business_package, workspace_root};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct OwnedResourceDescriptor {
    pub name: String,
    pub title: String,
    pub table: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    pub database: OwnedResourceDatabase,
    pub model_module: String,
    pub model_type: String,
    pub route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontend_page: Option<String>,
    /// 数据范围字段；由 `#[resource(owner_field = "...")]` 声明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_field: Option<String>,
    pub menu_parent: Option<String>,
    pub menu_order: u16,
    pub primary_key: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<OwnedIndexDescriptor>,
    pub fields: Vec<OwnedFieldDescriptor>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct OwnedIndexDescriptor {
    pub name: String,
    pub fields: Vec<String>,
    pub unique: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OwnedResourceDatabase {
    Control,
    Tenant,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct OwnedFieldDescriptor {
    pub name: String,
    pub column: String,
    pub rust_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_on_update: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    pub rename_from: Option<String>,
    pub default: Option<String>,
    pub primary_key: bool,
    pub generated: bool,
    pub auto_increment: bool,
    pub read_only: bool,
    pub filter: bool,
    pub sort: bool,
    pub unique: bool,
}
