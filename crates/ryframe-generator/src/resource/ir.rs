use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{ResourceError, spec::*};

mod contract;
mod permission;
mod schema;
mod value;

use contract::{validate_generation_contract, validate_storage};
use permission::is_permission;
use schema::normalize_schema;
use value::{validate_enum_key, validate_field_value, validate_ir_value};

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

pub(super) fn normalize(
    mut spec: ResourceSpec,
    source_path: String,
    source_hash: String,
) -> Result<ResourceIr, ResourceError> {
    let resource = spec.resource.name.clone();
    let error = |message: String, suggestion: &'static str| {
        ResourceError::new(message, suggestion)
            .with_resource(&resource)
            .with_file(&source_path)
    };

    if spec.schema_version != 1 {
        return Err(error(
            format!("不支持 schema_version {}", spec.schema_version),
            "将 schema_version 设置为 1",
        ));
    }
    if !is_snake_identifier(&resource) {
        return Err(error(
            "资源名必须是小写 snake_case 标识符".into(),
            "例如使用 `device` 或 `work_order`",
        ));
    }
    if !is_snake_identifier(&spec.resource.module) {
        return Err(error(
            format!("模块名 `{}` 不是安全标识符", spec.resource.module),
            "模块名只使用小写字母、数字和下划线",
        ));
    }
    if !is_snake_identifier(&spec.database.table) {
        return Err(error(
            format!("表名 `{}` 不是安全标识符", spec.database.table),
            "表名只使用小写字母、数字和下划线",
        ));
    }
    validate_labels(&spec.resource.labels, &resource, None, &source_path)?;
    validate_path_matches_resource(&source_path, &resource)?;
    let schema_hash = normalize_schema(&spec, &resource, &source_path)?;

    if spec.fields.is_empty() {
        return Err(error(
            "资源没有字段".into(),
            "至少声明主键和一个可读业务字段",
        ));
    }

    let mut names = BTreeSet::new();
    let mut orders = BTreeSet::new();
    let input_fields = std::mem::take(&mut spec.fields);
    let mut fields = Vec::with_capacity(input_fields.len());
    for field in input_fields {
        validate_field(&resource, &source_path, &field)?;
        if !names.insert(field.name.clone()) {
            return Err(field_error(
                &resource,
                &field.name,
                &source_path,
                "字段名重复",
                "删除重复字段或为字段改用唯一名称",
            ));
        }
        if !orders.insert(field.order) {
            return Err(field_error(
                &resource,
                &field.name,
                &source_path,
                format!("字段顺序 {} 重复", field.order),
                "为每个字段分配唯一 order",
            ));
        }
        let nullable = field.nullable;
        let wire_type = field.wire_type.unwrap_or(field.value_type);
        fields.push(FieldIr {
            name: field.name,
            value_type: field.value_type,
            wire_type,
            rust_type: field.value_type.rust_type(nullable),
            typescript_type: wire_type.typescript_type(nullable),
            order: field.order,
            nullable,
            default: field.default,
            usage: FieldUsageIr {
                create: field.usage.create,
                create_optional: field.usage.create_optional,
                update: field.usage.update,
                update_optional: field.usage.update_optional,
                read: field.usage.read,
                list: field.usage.list,
                filter: field.usage.filter,
                sort: field.usage.sort,
            },
            validation: ValidationIr {
                required: field.validation.required,
                min_length: field.validation.min_length,
                max_length: field.validation.max_length,
                minimum: field.validation.minimum,
                maximum: field.validation.maximum,
                pattern: field.validation.pattern,
            },
            labels: labels(field.labels),
            widget: widget(field.widget),
            enum_values: field
                .enum_values
                .into_iter()
                .map(|(key, labels)| {
                    (
                        key,
                        LabelsIr {
                            zh_cn: labels.zh_cn,
                            en: labels.en,
                        },
                    )
                })
                .collect(),
        });
    }
    fields.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then(left.name.cmp(&right.name))
    });

    validate_references(&resource, &source_path, &spec, &names, &fields)?;
    validate_storage(&resource, &source_path, &spec, &names)?;
    validate_api_and_access(&resource, &source_path, &spec)?;
    validate_generation_contract(&resource, &source_path, &spec, &fields)?;
    let extension_permissions = validate_extensions(
        &resource,
        &source_path,
        "backend",
        &spec.extensions.backend,
        true,
    )?;
    validate_extensions(
        &resource,
        &source_path,
        "frontend",
        &spec.extensions.frontend,
        false,
    )?;

    let mut indexes = spec
        .database
        .indexes
        .into_iter()
        .map(|index| IndexIr {
            name: index.name,
            fields: index.fields,
            unique: index.unique,
        })
        .collect::<Vec<_>>();
    indexes.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(ResourceIr {
        schema_version: spec.schema_version,
        pascal_name: crate::naming::to_pascal_case(&resource),
        name: resource,
        module: spec.resource.module,
        profile: spec.resource.profile,
        storage: spec.storage.kind,
        tenant_field: spec.storage.tenant_field,
        table: spec.database.table,
        primary_key: spec.database.primary_key,
        bootstrap_migration: spec.database.bootstrap_migration,
        schema_hash,
        schema_revision: spec.database.schema_revision,
        indexes,
        soft_delete: spec.database.soft_delete.map(|value| SoftDeleteIr {
            field: value.field,
            active: value.active,
            deleted: value.deleted,
        }),
        audit: spec.database.audit.map(|value| AuditIr {
            created_at: value.created_at,
            created_by: value.created_by,
            updated_at: value.updated_at,
            updated_by: value.updated_by,
        }),
        fields,
        api: ApiIr {
            path: spec.api.path,
            operations: operations(spec.api.operations),
        },
        access: AccessIr {
            capability: spec.access.capability,
            permissions: permissions(spec.access.permissions),
        },
        menu: MenuIr {
            key: spec.menu.key,
            parent: spec.menu.parent,
            order: spec.menu.order,
            icon: spec.menu.icon,
            labels: labels(spec.menu.labels),
        },
        route: RouteIr {
            key: spec.route.key,
            path: spec.route.path,
        },
        labels: labels(spec.resource.labels),
        extension_permissions,
        backend_extensions: spec.extensions.backend,
        frontend_extensions: spec.extensions.frontend,
        source_path,
        source_hash,
    })
}

fn validate_path_matches_resource(path: &str, resource: &str) -> Result<(), ResourceError> {
    let normalized = path.replace('\\', "/");
    if normalized.contains("/resources/") && !normalized.ends_with(&format!("/{resource}.toml")) {
        return Err(ResourceError::new(
            "清单文件名与资源名不一致",
            format!("将文件改名为 `{resource}.toml`，或同步修改 resource.name"),
        )
        .with_resource(resource)
        .with_file(path));
    }
    Ok(())
}

fn validate_field(
    resource: &str,
    source_path: &str,
    field: &FieldSpec,
) -> Result<(), ResourceError> {
    if !is_snake_identifier(&field.name) {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "字段名必须是小写 snake_case 标识符",
            "字段名只使用小写字母、数字和下划线",
        ));
    }
    if let Some(wire_type) = field.wire_type
        && wire_type != field.value_type
        && !matches!(
            (field.value_type, wire_type),
            (ValueType::I64, ValueType::String)
        )
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "不支持该 value_type 与 wire_type 组合",
            "目前只有 i64 可在 API 中安全序列化为 string",
        ));
    }
    validate_labels(&field.labels, resource, Some(&field.name), source_path)?;
    if field.value_type == ValueType::Decimal {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "flat_crud v1 不生成 decimal 字段",
            "将精度与舍入规则放入强类型业务切片，或先扩展完整 decimal 契约测试",
        ));
    }
    if field.validation.required && field.nullable {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "nullable 与 validation.required=true 冲突",
            "将字段改为非空，或取消 required",
        ));
    }
    if let (Some(min), Some(max)) = (field.validation.min_length, field.validation.max_length)
        && min > max
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "min_length 大于 max_length",
            "调整长度边界，使最小值不大于最大值",
        ));
    }
    if let (Some(min), Some(max)) = (field.validation.minimum, field.validation.maximum)
        && min > max
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "minimum 大于 maximum",
            "调整数值边界，使最小值不大于最大值",
        ));
    }
    if field.validation.pattern.is_some() {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "flat_crud v1 不生成 validation.pattern",
            "将正则语义放入强类型 DTO/frontend extension，并为两端添加一致性测试",
        ));
    }
    if (field.validation.min_length.is_some() || field.validation.max_length.is_some())
        && field.value_type != ValueType::String
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "字符串校验被用于非 string 字段",
            "移除长度校验，或将 value_type 改为 string",
        ));
    }
    if (field.validation.minimum.is_some() || field.validation.maximum.is_some())
        && !matches!(field.value_type, ValueType::I32 | ValueType::I64)
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "数值范围校验被用于非 i32/i64 字段",
            "移除 minimum/maximum，或使用 i32/i64；精确数值规则放入强类型扩展",
        ));
    }
    if (field.usage.create || field.usage.update)
        && !matches!(
            field.widget,
            WidgetSpec::Text | WidgetSpec::Number | WidgetSpec::Select
        )
    {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "可编辑字段使用了 FlatCrud 尚不支持的控件",
            "v1 仅支持 text、number、select；其他控件放入强类型 frontend extension",
        ));
    }
    if !field.enum_values.is_empty() && !matches!(field.widget, WidgetSpec::Select) {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "enum_values 只能配合 select 控件",
            "将 widget 改为 select，或删除 enum_values",
        ));
    }
    if matches!(field.widget, WidgetSpec::Select) && field.enum_values.is_empty() {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "select 控件缺少 enum_values",
            "声明有限枚举项，复杂选项加载放入前端扩展",
        ));
    }
    for (key, labels) in &field.enum_values {
        validate_labels(
            &LabelsSpec {
                zh_cn: labels.zh_cn.clone(),
                en: labels.en.clone(),
            },
            resource,
            Some(&field.name),
            source_path,
        )?;
        validate_enum_key(resource, source_path, field, key)?;
    }
    if field.usage.create_optional && !field.usage.create {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "create_optional 只能用于 create 字段",
            "启用 usage.create，或删除 create_optional",
        ));
    }
    if field.usage.update_optional && !field.usage.update {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "update_optional 只能用于 update 字段",
            "启用 usage.update，或删除 update_optional",
        ));
    }
    if field.nullable && field.usage.update_optional {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "nullable 与 update_optional 无法区分未提交和显式置空",
            "使用非可空可选更新，或在手写 DTO 中使用三态字段",
        ));
    }
    if field.nullable && field.usage.create_optional {
        return Err(field_error(
            resource,
            &field.name,
            source_path,
            "nullable 字段无需 create_optional，当前组合会丢失显式空值语义",
            "删除 create_optional；nullable 字段的创建 DTO 已经是 Option",
        ));
    }
    if let Some(default) = &field.default {
        validate_field_value(resource, source_path, field, default, "default")?;
    }
    Ok(())
}

fn validate_references(
    resource: &str,
    source_path: &str,
    spec: &ResourceSpec,
    fields: &BTreeSet<String>,
    field_specs: &[FieldIr],
) -> Result<(), ResourceError> {
    if spec.database.primary_key.is_empty() {
        return Err(
            ResourceError::new("主键不能为空", "在 database.primary_key 中声明字段")
                .with_resource(resource)
                .with_file(source_path),
        );
    }
    let mut primary = BTreeSet::new();
    for field in &spec.database.primary_key {
        ensure_field_reference(resource, source_path, fields, field, "主键")?;
        if !primary.insert(field) {
            return Err(field_error(
                resource,
                field,
                source_path,
                "主键字段重复",
                "从 database.primary_key 删除重复项",
            ));
        }
    }
    let mut index_names = BTreeSet::new();
    for index in &spec.database.indexes {
        if !is_safe_symbol(&index.name) || index.fields.is_empty() {
            return Err(ResourceError::new(
                format!("索引 `{}` 名称无效或字段为空", index.name),
                "使用安全索引名并至少声明一个字段",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
        if !index_names.insert(&index.name) {
            return Err(ResourceError::new(
                format!("索引 `{}` 重复", index.name),
                "为每个索引使用唯一名称",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
        let mut index_fields = BTreeSet::new();
        for field in &index.fields {
            ensure_field_reference(resource, source_path, fields, field, "索引")?;
            if !index_fields.insert(field) {
                return Err(field_error(
                    resource,
                    field,
                    source_path,
                    format!("索引 `{}` 中字段重复", index.name),
                    "删除索引中的重复字段",
                ));
            }
        }
    }
    if let Some(soft_delete) = &spec.database.soft_delete {
        ensure_field_reference(resource, source_path, fields, &soft_delete.field, "软删除")?;
        let field = field_specs
            .iter()
            .find(|field| field.name == soft_delete.field)
            .expect("字段引用已校验");
        validate_ir_value(
            resource,
            source_path,
            field,
            &soft_delete.active,
            "soft_delete.active",
        )?;
        validate_ir_value(
            resource,
            source_path,
            field,
            &soft_delete.deleted,
            "soft_delete.deleted",
        )?;
        if soft_delete.active == soft_delete.deleted {
            return Err(field_error(
                resource,
                &soft_delete.field,
                source_path,
                "软删除 active 与 deleted 值相同",
                "为有效和删除状态使用不同值",
            ));
        }
    }
    if let Some(audit) = &spec.database.audit {
        for field in [
            Some(&audit.created_at),
            audit.created_by.as_ref(),
            Some(&audit.updated_at),
            audit.updated_by.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            ensure_field_reference(resource, source_path, fields, field, "审计")?;
        }
    }
    Ok(())
}

fn validate_api_and_access(
    resource: &str,
    source_path: &str,
    spec: &ResourceSpec,
) -> Result<(), ResourceError> {
    for (label, path) in [("API", &spec.api.path), ("路由", &spec.route.path)] {
        if !is_safe_route(path) {
            return Err(ResourceError::new(
                format!("{label}路径 `{path}` 无效"),
                "路径必须以 / 开头，且不能包含空白、查询串或父目录片段",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
    }
    let operations = operations(spec.api.operations.clone());
    let operation_values = operations.values();
    if operation_values
        .iter()
        .any(|value| !is_operation_symbol(value))
    {
        return Err(ResourceError::new(
            "operationId 必须是安全标识符",
            "使用 OpenAPI 中的精确 operationId，例如 `get_system_devices`",
        )
        .with_resource(resource)
        .with_file(source_path));
    }
    if operation_values.into_iter().collect::<BTreeSet<_>>().len() != 5 {
        return Err(ResourceError::new(
            "operationId 不能重复",
            "为 create/read/list/update/delete 分配唯一 operationId",
        )
        .with_resource(resource)
        .with_file(source_path));
    }
    let permission_values = permissions(spec.access.permissions.clone());
    if permission_values
        .values()
        .iter()
        .any(|value| !is_permission(value))
    {
        return Err(ResourceError::new(
            "权限标识格式无效",
            "使用 `module:resource-name:action-name` 形式的三段 kebab-case 权限标识",
        )
        .with_resource(resource)
        .with_file(source_path));
    }
    for (label, value) in [
        ("capability", &spec.access.capability),
        ("menu.key", &spec.menu.key),
        ("menu.parent", &spec.menu.parent),
        ("route.key", &spec.route.key),
    ] {
        if !is_safe_symbol(value) {
            return Err(ResourceError::new(
                format!("{label} `{value}` 格式无效"),
                "只使用字母、数字、下划线、短横线或点",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
    }
    if let Some(icon) = &spec.menu.icon
        && !is_safe_symbol(icon)
    {
        return Err(ResourceError::new(
            format!("menu.icon `{icon}` 格式无效"),
            "只使用字母、数字、下划线、短横线或点",
        )
        .with_resource(resource)
        .with_file(source_path));
    }
    validate_labels(&spec.menu.labels, resource, None, source_path)
}

fn validate_extensions(
    resource: &str,
    source_path: &str,
    side: &str,
    extensions: &BTreeMap<String, toml::Value>,
    collect_permissions: bool,
) -> Result<BTreeMap<String, String>, ResourceError> {
    let mut permissions = BTreeMap::new();
    for (key, value) in extensions {
        if !is_snake_identifier(key) {
            return Err(ResourceError::new(
                format!("{side} 扩展键 `{key}` 格式无效"),
                "扩展键使用小写 snake_case；复杂行为放入同名强类型扩展文件",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
        if collect_permissions && let Some(name) = key.strip_suffix("_permission") {
            if name.is_empty() || !is_snake_identifier(name) {
                return Err(ResourceError::new(
                    format!("扩展权限名 `{key}` 格式无效"),
                    "使用 `<action>_permission`，其中 action 为小写 snake_case",
                )
                .with_resource(resource)
                .with_file(source_path));
            }
            let toml::Value::String(permission) = value else {
                return Err(ResourceError::new(
                    format!("扩展权限 `{key}` 必须是字符串"),
                    "填写 `module:resource:action` 形式的权限字符串",
                )
                .with_resource(resource)
                .with_file(source_path));
            };
            if !is_permission(permission) {
                return Err(ResourceError::new(
                    format!("扩展权限 `{key}` 的值格式无效"),
                    "使用 `module:resource-name:action-name` 形式的三段 kebab-case 权限标识",
                )
                .with_resource(resource)
                .with_file(source_path));
            }
            permissions.insert(name.to_owned(), permission.clone());
        }
    }
    Ok(permissions)
}

fn ensure_field_reference(
    resource: &str,
    source_path: &str,
    fields: &BTreeSet<String>,
    field: &str,
    owner: &str,
) -> Result<(), ResourceError> {
    if fields.contains(field) {
        Ok(())
    } else {
        Err(field_error(
            resource,
            field,
            source_path,
            format!("{owner}引用了未声明字段"),
            "补充 fields 条目，或修正引用名称",
        ))
    }
}

fn validate_labels(
    labels: &LabelsSpec,
    resource: &str,
    field: Option<&str>,
    source_path: &str,
) -> Result<(), ResourceError> {
    if labels.zh_cn.trim().is_empty() || labels.en.trim().is_empty() {
        let mut error =
            ResourceError::new("中英文标签均不能为空", "同时填写 labels.zh_cn 和 labels.en")
                .with_resource(resource)
                .with_file(source_path);
        if let Some(field) = field {
            error = error.with_field(field);
        }
        return Err(error);
    }
    Ok(())
}

fn field_error(
    resource: &str,
    field: &str,
    source_path: &str,
    message: impl Into<String>,
    suggestion: impl Into<String>,
) -> ResourceError {
    ResourceError::new(message, suggestion)
        .with_resource(resource)
        .with_field(field)
        .with_file(source_path)
}

fn labels(value: LabelsSpec) -> LabelsIr {
    LabelsIr {
        zh_cn: value.zh_cn,
        en: value.en,
    }
}

fn widget(value: WidgetSpec) -> WidgetIr {
    match value {
        WidgetSpec::Text => WidgetIr::Text,
        WidgetSpec::Textarea => WidgetIr::Textarea,
        WidgetSpec::Number => WidgetIr::Number,
        WidgetSpec::Switch => WidgetIr::Switch,
        WidgetSpec::Select => WidgetIr::Select,
        WidgetSpec::Date => WidgetIr::Date,
        WidgetSpec::DateTime => WidgetIr::DateTime,
        WidgetSpec::Hidden => WidgetIr::Hidden,
    }
}

fn operations(value: OperationSpec) -> PermissionIr {
    PermissionIr {
        create: value.create,
        read: value.read,
        list: value.list,
        update: value.update,
        delete: value.delete,
    }
}

fn permissions(value: PermissionSpec) -> PermissionIr {
    PermissionIr {
        create: value.create,
        read: value.read,
        list: value.list,
        update: value.update,
        delete: value.delete,
    }
}

fn is_snake_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && !value.ends_with('_')
        && !value.contains("__")
}

fn is_operation_symbol(value: &str) -> bool {
    matches!(value.bytes().next(), Some(b'a'..=b'z' | b'A'..=b'Z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn is_safe_symbol(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn is_safe_route(value: &str) -> bool {
    value.starts_with('/')
        && value.len() > 1
        && !value.contains("..")
        && !value.contains('?')
        && !value.contains('#')
        && !value.chars().any(char::is_whitespace)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'{' | b'}' | b':')
        })
}
