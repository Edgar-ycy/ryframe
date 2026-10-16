use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceDatabase {
    Control,
    Tenant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResourceFieldDescriptor {
    pub name: &'static str,
    pub column: &'static str,
    pub rust_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_default: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_on_update: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_comment: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collation: Option<&'static str>,
    pub rename_from: Option<&'static str>,
    pub default: Option<&'static str>,
    pub primary_key: bool,
    pub generated: bool,
    pub auto_increment: bool,
    pub read_only: bool,
    pub filter: bool,
    pub sort: bool,
    pub unique: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResourceDescriptor {
    pub name: &'static str,
    pub title: &'static str,
    pub table: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collation: Option<&'static str>,
    pub database: ResourceDatabase,
    pub model_module: &'static str,
    pub model_type: &'static str,
    pub route: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontend_page: Option<&'static str>,
    pub menu_parent: Option<&'static str>,
    pub menu_order: u16,
    pub primary_key: &'static [&'static str],
    pub indexes: &'static [ResourceIndexDescriptor],
    pub fields: &'static [ResourceFieldDescriptor],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResourceIndexDescriptor {
    pub name: &'static str,
    pub fields: &'static [&'static str],
    pub unique: bool,
}

pub trait ResourceModel {
    fn descriptor() -> ResourceDescriptor;
}
