//! 使用者业务 crate 的发现、描述符读取和生成入口。

mod bootstrap;
mod catalog;
mod format;
mod generate;
mod metadata;
mod migrations;
mod model;
pub(crate) mod writer;
pub(crate) use bootstrap::validate_module;

pub use bootstrap::{
    BusinessBootstrapOptions, BusinessBootstrapReport, bootstrap_business_package,
};
pub use generate::{BusinessGenerateOptions, BusinessGenerateReport, generate_business_package};
pub use metadata::{BusinessPackage, locate_business_package, workspace_root};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct OwnedResourceDescriptor {
    pub name: String,
    pub title: String,
    pub table: String,
    pub database: OwnedResourceDatabase,
    pub model_module: String,
    pub model_type: String,
    pub route: Option<String>,
    pub menu_parent: Option<String>,
    pub menu_order: u16,
    pub primary_key: Vec<String>,
    pub fields: Vec<OwnedFieldDescriptor>,
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
