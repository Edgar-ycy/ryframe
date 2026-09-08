use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{RelationKind, ResourceError, ResourceProfile, StorageKind, ValueType};

/// 资源清单的稳定输入模型。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceSpec {
    pub schema_version: u16,
    pub resource: ResourceIdentitySpec,
    pub storage: StorageSpec,
    pub database: DatabaseSpec,
    pub fields: Vec<FieldSpec>,
    #[serde(default)]
    pub relations: Vec<RelationSpec>,
    pub api: ApiSpec,
    pub access: AccessSpec,
    pub menu: MenuSpec,
    pub route: RouteSpec,
    #[serde(default)]
    pub extensions: ExtensionSpec,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RelationSpec {
    pub name: String,
    pub kind: RelationKind,
    pub local_field: String,
    pub target_resource: String,
}

impl ResourceSpec {
    pub fn parse(source: &str, source_path: &str) -> Result<Self, ResourceError> {
        let value = toml::from_str::<toml::Value>(source).map_err(|error| {
            ResourceError::new(
                format!("TOML 语法错误：{error}"),
                "按照错误中的行列位置修正引号、数组或表头",
            )
            .with_file(source_path)
        })?;
        reject_forbidden_dsl(&value, source_path, "")?;
        value.try_into().map_err(|error| {
            ResourceError::new(
                format!("资源清单结构错误：{error}"),
                "删除未知键，或按资源清单 schema 补齐必填字段",
            )
            .with_file(source_path)
        })
    }
}

fn reject_forbidden_dsl(
    value: &toml::Value,
    source_path: &str,
    parent: &str,
) -> Result<(), ResourceError> {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                let path = if parent.is_empty() {
                    key.clone()
                } else {
                    format!("{parent}.{key}")
                };
                let normalized = key.replace('-', "_").to_ascii_lowercase();
                if matches!(
                    normalized.as_str(),
                    "sql"
                        | "raw_sql"
                        | "query_sql"
                        | "layout"
                        | "page_layout"
                        | "state_machine"
                        | "workflow"
                        | "cross_resource_transaction"
                ) {
                    return Err(ResourceError::new(
                        format!("不允许在资源清单中声明 `{path}`"),
                        "将复杂 SQL、页面布局、状态机或跨资源事务放入强类型手写扩展",
                    )
                    .with_file(source_path));
                }
                reject_forbidden_dsl(child, source_path, &path)?;
            }
        }
        toml::Value::Array(values) => {
            for child in values {
                reject_forbidden_dsl(child, source_path, parent)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceIdentitySpec {
    pub name: String,
    pub module: String,
    pub profile: ResourceProfile,
    #[serde(default)]
    pub labels: LabelsSpec,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StorageSpec {
    pub kind: StorageKind,
    #[serde(default)]
    pub tenant_field: Option<String>,
    #[serde(default = "default_true")]
    pub configuration_versioned: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseSpec {
    pub table: String,
    pub primary_key: Vec<String>,
    /// 为新资源生成首次引入的不可变初始迁移。
    #[serde(default)]
    pub bootstrap_migration: bool,
    /// 后续 schema 演进对应的显式追加迁移名称；首次引入可以省略。
    #[serde(default)]
    pub schema_revision: Option<String>,
    #[serde(default)]
    pub indexes: Vec<IndexSpec>,
    #[serde(default)]
    pub soft_delete: Option<SoftDeleteSpec>,
    #[serde(default)]
    pub audit: Option<AuditSpec>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexSpec {
    pub name: String,
    pub fields: Vec<String>,
    #[serde(default)]
    pub unique: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SoftDeleteSpec {
    pub field: String,
    pub active: toml::Value,
    pub deleted: toml::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditSpec {
    pub created_at: String,
    #[serde(default)]
    pub created_by: Option<String>,
    pub updated_at: String,
    #[serde(default)]
    pub updated_by: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSpec {
    pub name: String,
    #[serde(default)]
    pub column: Option<String>,
    pub value_type: ValueType,
    #[serde(default)]
    pub wire_type: Option<ValueType>,
    pub order: u16,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub default: Option<toml::Value>,
    pub usage: FieldUsageSpec,
    #[serde(default)]
    pub validation: ValidationSpec,
    pub labels: LabelsSpec,
    pub widget: WidgetSpec,
    #[serde(default)]
    pub enum_values: BTreeMap<String, EnumValueSpec>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FieldUsageSpec {
    #[serde(default)]
    pub create: bool,
    #[serde(default)]
    pub create_optional: bool,
    #[serde(default)]
    pub update: bool,
    #[serde(default)]
    pub update_optional: bool,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub list: bool,
    #[serde(default)]
    pub filter: bool,
    #[serde(default)]
    pub filter_exact: bool,
    #[serde(default)]
    pub sort: bool,
    #[serde(default)]
    pub sort_desc: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationSpec {
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub min_length: Option<u32>,
    #[serde(default)]
    pub max_length: Option<u32>,
    #[serde(default)]
    pub min_utf8_bytes: Option<u32>,
    #[serde(default)]
    pub max_utf8_bytes: Option<u32>,
    #[serde(default)]
    pub minimum: Option<i64>,
    #[serde(default)]
    pub maximum: Option<i64>,
    #[serde(default)]
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LabelsSpec {
    #[serde(default)]
    pub zh_cn: String,
    #[serde(default)]
    pub en: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetSpec {
    Text,
    Textarea,
    Number,
    Switch,
    Select,
    Date,
    DateTime,
    Hidden,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnumValueSpec {
    pub zh_cn: String,
    pub en: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSpec {
    pub path: String,
    pub operations: OperationSpec,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationSpec {
    pub create: String,
    pub read: String,
    pub list: String,
    pub update: String,
    pub delete: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessSpec {
    #[serde(default)]
    pub capability: Option<String>,
    #[serde(default)]
    pub owner_field: Option<String>,
    pub permissions: PermissionSpec,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionSpec {
    pub create: String,
    pub read: String,
    pub list: String,
    pub update: String,
    pub delete: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MenuSpec {
    pub key: String,
    pub parent: String,
    pub order: u16,
    #[serde(default)]
    pub icon: Option<String>,
    pub labels: LabelsSpec,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RouteSpec {
    pub key: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionSpec {
    #[serde(default)]
    pub backend: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub frontend: BTreeMap<String, toml::Value>,
}
