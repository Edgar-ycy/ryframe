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
    pub rename_from: Option<&'static str>,
    pub default: Option<&'static str>,
    pub primary_key: bool,
    pub generated: bool,
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
    pub database: ResourceDatabase,
    pub model_module: &'static str,
    pub model_type: &'static str,
    pub route: Option<&'static str>,
    pub menu_parent: Option<&'static str>,
    pub menu_order: u16,
    pub fields: &'static [ResourceFieldDescriptor],
}

pub trait ResourceModel {
    fn descriptor() -> ResourceDescriptor;
}
