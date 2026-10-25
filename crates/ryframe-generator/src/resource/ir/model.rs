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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdGeneration {
    #[default]
    Snowflake,
    AutoIncrement,
}

impl IdGeneration {
    pub(crate) fn is_snowflake(&self) -> bool {
        *self == Self::Snowflake
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    String,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
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
    pub fn rust_base_type(self) -> &'static str {
        match self {
            Self::String => "String",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::Decimal => "rust_decimal::Decimal",
            Self::Bool => "bool",
            Self::Date => "chrono::NaiveDate",
            Self::DateTime => "chrono::DateTime<chrono::Utc>",
            Self::Json => "serde_json::Value",
        }
    }

    pub fn integer_bounds(self) -> Option<(i128, i128)> {
        Some(match self {
            Self::I8 => (i8::MIN.into(), i8::MAX.into()),
            Self::I16 => (i16::MIN.into(), i16::MAX.into()),
            Self::I32 => (i32::MIN.into(), i32::MAX.into()),
            Self::I64 => (i64::MIN.into(), i64::MAX.into()),
            Self::U8 => (0, u8::MAX.into()),
            Self::U16 => (0, u16::MAX.into()),
            Self::U32 => (0, u32::MAX.into()),
            Self::U64 => (0, u64::MAX.into()),
            Self::String
            | Self::Decimal
            | Self::Bool
            | Self::Date
            | Self::DateTime
            | Self::Json => return None,
        })
    }

    pub fn rust_type(self, nullable: bool) -> String {
        let value = self.rust_base_type();
        if nullable {
            format!("Option<{value}>")
        } else {
            value.to_owned()
        }
    }

    pub fn typescript_type(self, nullable: bool) -> String {
        let value = match self {
            Self::String | Self::Date | Self::DateTime | Self::Decimal | Self::U64 => "string",
            Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::U8 | Self::U16 | Self::U32 => {
                "number"
            }
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
    pub configuration_versioned: bool,
    pub table: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    pub primary_key: Vec<String>,
    #[serde(skip_serializing_if = "IdGeneration::is_snowflake")]
    pub id_generation: IdGeneration,
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

impl ResourceIr {
    pub fn column(&self, field: &str) -> &str {
        self.fields
            .iter()
            .find(|candidate| candidate.name == field)
            .map(|candidate| candidate.column.as_str())
            .expect("字段引用已由 IR 校验")
    }
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
    pub column: String,
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

impl FieldIr {
    pub(crate) fn database_clock(&self) -> bool {
        self.sql_default.is_some() || self.sql_on_update.is_some()
    }

    pub(crate) fn database_default(&self) -> bool {
        !self.usage.create && self.sql_default.is_some()
    }

    pub(crate) fn database_update(&self) -> bool {
        !self.usage.update && self.database_clock()
    }
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
    #[serde(skip_serializing_if = "is_false")]
    pub filter_exact: bool,
    pub sort: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub sort_desc: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationIr {
    pub required: bool,
    pub min_length: Option<u32>,
    pub max_length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_utf8_bytes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_utf8_bytes: Option<u32>,
    pub minimum: Option<i64>,
    pub maximum: Option<i64>,
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetIr {
    Text,
    Textarea,
    Number,
    Switch,
    Select,
    Date,
    DateTime,
    Json,
    Hidden,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiIr {
    pub path: String,
    pub operations: PermissionIr,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccessIr {
    #[serde(skip_serializing_if = "is_false")]
    pub system_tenant_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_field: Option<String>,
    pub permissions: PermissionIr,
}

fn is_false(value: &bool) -> bool {
    !*value
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
