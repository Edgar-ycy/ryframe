use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceProfile {
    FlatCrud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    ControlRow,
    TenantData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    String,
    I32,
    I64,
    Decimal,
    Bool,
    Date,
    DateTime,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    BelongsTo,
}

impl ValueType {
    pub fn rust_type(self, nullable: bool) -> String {
        let value = match self {
            Self::String => "String",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::Decimal => "rust_decimal::Decimal",
            Self::Bool => "bool",
            Self::Date => "chrono::NaiveDate",
            Self::DateTime => "chrono::DateTime<chrono::Utc>",
            Self::Json => "serde_json::Value",
        };
        if nullable {
            format!("Option<{value}>")
        } else {
            value.to_owned()
        }
    }

    pub fn typescript_type(self, nullable: bool) -> String {
        let value = match self {
            Self::String | Self::Date | Self::DateTime | Self::Decimal => "string",
            Self::I32 | Self::I64 => "number",
            Self::Bool => "boolean",
            Self::Json => "unknown",
        };
        if nullable {
            format!("{value} | null")
        } else {
            value.to_owned()
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceIr {
    pub schema_version: u16,
    pub name: String,
    pub pascal_name: String,
    pub module: String,
    pub profile: ResourceProfile,
    pub storage: StorageKind,
    pub tenant_field: Option<String>,
    pub table: String,
    pub primary_key: Vec<String>,
    pub bootstrap_migration: bool,
    pub schema_hash: String,
    pub schema_revision: Option<String>,
    pub indexes: Vec<IndexIr>,
    pub soft_delete: Option<SoftDeleteIr>,
    pub audit: Option<AuditIr>,
    pub fields: Vec<FieldIr>,
    pub relations: Vec<RelationIr>,
    pub api: ApiIr,
    pub access: AccessIr,
    pub menu: MenuIr,
    pub route: RouteIr,
    pub labels: LabelsIr,
    pub extension_permissions: BTreeMap<String, String>,
    pub backend_extensions: BTreeMap<String, toml::Value>,
    pub frontend_extensions: BTreeMap<String, toml::Value>,
    pub source_path: String,
    pub source_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelationIr {
    pub name: String,
    pub pascal_name: String,
    pub kind: RelationKind,
    pub local_field: String,
    pub target_resource: String,
    pub target_pascal_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexIr {
    pub name: String,
    pub fields: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SoftDeleteIr {
    pub field: String,
    pub active: toml::Value,
    pub deleted: toml::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditIr {
    pub created_at: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldIr {
    pub name: String,
    pub value_type: ValueType,
    pub wire_type: ValueType,
    pub rust_type: String,
    pub typescript_type: String,
    pub order: u16,
    pub nullable: bool,
    pub default: Option<toml::Value>,
    pub usage: FieldUsageIr,
    pub validation: ValidationIr,
    pub labels: LabelsIr,
    pub widget: WidgetIr,
    pub enum_values: BTreeMap<String, LabelsIr>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldUsageIr {
    pub create: bool,
    pub create_optional: bool,
    pub update: bool,
    pub update_optional: bool,
    pub read: bool,
    pub list: bool,
    pub filter: bool,
    pub sort: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationIr {
    pub required: bool,
    pub min_length: Option<u32>,
    pub max_length: Option<u32>,
    pub minimum: Option<i64>,
    pub maximum: Option<i64>,
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetIr {
    Text,
    Textarea,
    Number,
    Switch,
    Select,
    Date,
    DateTime,
    Hidden,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiIr {
    pub path: String,
    pub operations: PermissionIr,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccessIr {
    pub capability: String,
    pub permissions: PermissionIr,
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionIr {
    pub create: String,
    pub read: String,
    pub list: String,
    pub update: String,
    pub delete: String,
}

impl PermissionIr {
    pub fn values(&self) -> [&str; 5] {
        [
            &self.create,
            &self.read,
            &self.list,
            &self.update,
            &self.delete,
        ]
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MenuIr {
    pub key: String,
    pub parent: String,
    pub order: u16,
    pub icon: Option<String>,
    pub labels: LabelsIr,
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteIr {
    pub key: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabelsIr {
    pub zh_cn: String,
    pub en: String,
}
