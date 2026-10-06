//! 使用者业务 crate 的发现、描述符读取和生成入口。

mod bootstrap;
mod catalog;
mod generate;
mod metadata;
mod model;
mod writer;

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
    pub rename_from: Option<String>,
    pub default: Option<String>,
    pub primary_key: bool,
    pub generated: bool,
    pub read_only: bool,
    pub filter: bool,
    pub sort: bool,
    pub unique: bool,
}
