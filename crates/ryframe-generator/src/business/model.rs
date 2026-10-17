use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use super::{
    BusinessPackage, OwnedFieldDescriptor, OwnedResourceDatabase, OwnedResourceDescriptor,
};
use crate::{
    AccessSpec, ApiSpec, AuditSpec, DatabaseSpec, ExtensionSpec, FieldSpec, FieldUsageSpec,
    IndexSpec, LabelsSpec, MenuSpec, OperationSpec, PermissionSpec, ResourceError,
    ResourceIdentitySpec, ResourceIr, ResourceProfile, ResourceSpec, RouteSpec, SoftDeleteSpec,
    StorageKind, StorageSpec, ValidationSpec, ValueType, WidgetSpec, normalize_resource,
};
pub(super) fn selected_names(
    descriptors: &[OwnedResourceDescriptor],
    models: &[String],
) -> Result<BTreeSet<String>, ResourceError> {
    if models.is_empty() {
        return Ok(descriptors.iter().map(|item| item.name.clone()).collect());
    }
    models
        .iter()
        .map(|model| {
            descriptors
                .iter()
                .find(|item| {
                    item.name.eq_ignore_ascii_case(model)
                        || item.model_type.eq_ignore_ascii_case(model)
                })
                .map(|selected| selected.name.clone())
                .ok_or_else(|| error(format!("业务模型 `{model}` 不存在")))
        })
        .collect()
}

pub(super) fn descriptor_to_ir(
    package: &BusinessPackage,
    descriptor: &OwnedResourceDescriptor,
) -> Result<ResourceIr, ResourceError> {
    validate_descriptor(package, descriptor)?;
    let owner_field = owner_field(descriptor)?;
    let mut fields = descriptor
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| field_spec(field, index))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(owner_field) = &owner_field {
        let field = fields
            .iter_mut()
            .find(|field| field.name == *owner_field)
            .expect("owner_field 已在描述符校验中存在");
        field.usage.create = false;
        field.usage.create_optional = false;
        field.usage.update = false;
        field.usage.update_optional = false;
        field.default = Some(toml::Value::Integer(0));
        field.widget = WidgetSpec::Hidden;
    }
    let primary_key = descriptor.primary_key.clone();
    let indexes = indexes(descriptor);
    let soft_delete = soft_delete(descriptor)?;
    let audit = audit(descriptor);
    let module = package.module.replace('-', "_");
    let permission_module = package.module.replace('_', "-");
    let permission_name = descriptor.name.replace('_', "-");
    let route_key = format!("{permission_module}.{permission_name}");
    let api_name = format!("{}_{}", module, descriptor.name);
    let labels = LabelsSpec {
        zh_cn: descriptor.title.clone(),
        en: descriptor.model_type.clone(),
    };
    let mut frontend = BTreeMap::new();
    if let Some(page) = &descriptor.frontend_page {
        frontend.insert("page".into(), toml::Value::String(page.clone()));
    }
    let spec = ResourceSpec {
        schema_version: 1,
        resource: ResourceIdentitySpec {
            name: descriptor.name.clone(),
            module: "business".into(),
            profile: ResourceProfile::FlatCrud,
            labels: labels.clone(),
        },
        storage: StorageSpec {
            kind: match descriptor.database {
                OwnedResourceDatabase::Control => StorageKind::ControlRow,
                OwnedResourceDatabase::Tenant => StorageKind::TenantData,
            },
            tenant_field: (descriptor.database == OwnedResourceDatabase::Tenant
                && descriptor
                    .fields
                    .iter()
                    .any(|field| field.name == "tenant_id"))
            .then(|| "tenant_id".into()),
            configuration_versioned: false,
        },
        database: DatabaseSpec {
            id_generation: if descriptor
                .fields
                .iter()
                .any(|field| field.name == "id" && field.auto_increment)
            {
                crate::IdGeneration::AutoIncrement
            } else {
                crate::IdGeneration::Snowflake
            },
            table: descriptor.table.clone(),
            collation: descriptor.collation.clone(),
            primary_key,
            bootstrap_migration: true,
            schema_revision: None,
            indexes,
            soft_delete,
            audit,
        },
        fields,
        relations: Vec::new(),
        api: ApiSpec {
            path: format!("/api/v1/business/{module}/{}", descriptor.name),
            operations: OperationSpec {
                create: format!("post_business_{api_name}"),
                read: format!("get_business_{api_name}_by_id"),
                list: format!("get_business_{api_name}"),
                update: format!("put_business_{api_name}_by_id"),
                delete: format!("delete_business_{api_name}_by_id"),
            },
        },
        access: AccessSpec {
            system_tenant_only: descriptor.database == OwnedResourceDatabase::Control,
            capability: None,
            owner_field,
            permissions: PermissionSpec {
                create: format!("{permission_module}:{permission_name}:add"),
                read: format!("{permission_module}:{permission_name}:list"),
                list: format!("{permission_module}:{permission_name}:list"),
                update: format!("{permission_module}:{permission_name}:edit"),
                delete: format!("{permission_module}:{permission_name}:remove"),
            },
        },
        menu: MenuSpec {
            key: route_key.clone(),
            parent: descriptor
                .menu_parent
                .clone()
                .unwrap_or_else(|| permission_module.clone()),
            order: descriptor.menu_order,
            icon: None,
            labels,
        },
        route: RouteSpec {
            key: route_key,
            path: descriptor
                .route
                .clone()
                .unwrap_or_else(|| format!("/{module}/{}", descriptor.name)),
        },
        extensions: ExtensionSpec {
            backend: BTreeMap::new(),
            frontend,
        },
    };
    let source = serde_json::to_vec(descriptor)
        .map_err(|serialize| error(format!("资源描述符序列化失败：{serialize}")))?;
    normalize_resource(
        spec,
        format!("{}::{}", descriptor.model_module, descriptor.model_type),
        hex::encode(Sha256::digest(source)),
    )
}

/// 数据范围字段来自 `#[resource(owner_field = "...")]`。
///
/// 控制库和租户库业务都可声明；控制库资源的系统租户专用约束由
/// `system_tenant_only` 继续执行，租户库资源仍按 `tenant_id` 隔离。
fn owner_field(descriptor: &OwnedResourceDescriptor) -> Result<Option<String>, ResourceError> {
    let Some(name) = descriptor.owner_field.as_deref() else {
        return Ok(None);
    };
    if name.trim().is_empty() {
        return Err(error(format!(
            "资源 {} 的 owner_field 不能为空；填写保存用户 ID 的 i64 字段名，或删除该属性",
            descriptor.name
        )));
    }
    if name.trim() != name {
        return Err(error(format!(
            "资源 {} 的 owner_field `{name}` 含有首尾空白；使用字段的 snake_case 名称，例如 owner_id",
            descriptor.name
        )));
    }
    let field = descriptor.fields.iter().find(|field| field.name == name);
    let field = field.ok_or_else(|| {
        error(format!(
            "资源 {} 的 owner_field `{name}` 未在模型中声明；添加同名字段，或改为已有字段",
            descriptor.name
        ))
    })?;
    let (value_type, _) = value_type(&field.rust_type)?;
    if value_type != ValueType::I64 {
        return Err(error(format!(
            "资源 {} 的 owner_field `{name}` 必须是 i64 或 Option<i64> 字段；当前类型为 `{}`",
            descriptor.name, field.rust_type
        )));
    }
    Ok(Some(name.to_owned()))
}

fn validate_descriptor(
    package: &BusinessPackage,
    descriptor: &OwnedResourceDescriptor,
) -> Result<(), ResourceError> {
    if descriptor.fields.is_empty() {
        return Err(error(format!("资源 {} 没有字段", descriptor.name)));
    }
    let primary = descriptor
        .fields
        .iter()
        .filter(|field| field.primary_key)
        .count();
    if primary == 0 {
        return Err(error(format!(
            "资源 {} 必须用 #[resource(primary_key)] 标记主键",
            descriptor.name
        )));
    }
    let keys = descriptor
        .primary_key
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    let marked = descriptor
        .fields
        .iter()
        .filter(|field| field.primary_key)
        .map(|field| &field.name)
        .collect::<std::collections::BTreeSet<_>>();
    if keys.len() != descriptor.primary_key.len() || keys != marked {
        return Err(error(format!(
            "资源 {} 的主键顺序与字段标记不一致；修正 primary_key 列表",
            descriptor.name
        )));
    }
    if descriptor.database == OwnedResourceDatabase::Tenant
        && !descriptor
            .fields
            .iter()
            .any(|field| field.name == "tenant_id" && field.primary_key)
    {
        return Err(error(format!(
            "租户资源 {} 必须把 tenant_id 纳入主键",
            descriptor.name
        )));
    }
    if descriptor.model_module.split("::").next() != Some(package.name.replace('-', "_").as_str()) {
        return Err(error(format!(
            "资源 {} 的模型不属于 package {}",
            descriptor.name, package.name
        )));
    }
    Ok(())
}

fn field_spec(field: &OwnedFieldDescriptor, index: usize) -> Result<FieldSpec, ResourceError> {
    let (value_type, nullable) = value_type(&field.rust_type)?;
    let hidden = field.primary_key
        || field.name == "tenant_id"
        || matches!(
            field.name.as_str(),
            "created_at" | "updated_at" | "del_flag"
        );
    let mutable = !field.primary_key && !field.generated && !field.read_only && !hidden;
    Ok(FieldSpec {
        name: field.name.clone(),
        column: (field.column != field.name).then(|| field.column.clone()),
        sql_type: field.sql_type.clone(),
        sql_default: field.sql_default.clone(),
        sql_on_update: field.sql_on_update.clone(),
        sql_comment: field.sql_comment.clone(),
        collation: field.collation.clone(),
        value_type,
        wire_type: matches!(
            value_type,
            ValueType::I64 | ValueType::U64 | ValueType::Decimal
        )
        .then_some(ValueType::String),
        order: u16::try_from((index + 1) * 10).map_err(|_| error("字段数量超过生成上限"))?,
        nullable,
        default: field
            .default
            .as_deref()
            .map(|value| default_value(value, value_type))
            .transpose()?,
        usage: FieldUsageSpec {
            create: mutable,
            create_optional: mutable && !nullable && field.default.is_some(),
            update: mutable,
            update_optional: mutable && !nullable,
            read: field.name != "tenant_id" && field.name != "del_flag",
            list: !hidden || field.primary_key || field.name == "created_at",
            filter: field.filter,
            filter_exact: false,
            sort: field.sort,
            sort_desc: false,
        },
        validation: ValidationSpec {
            required: mutable && !nullable && field.default.is_none(),
            ..ValidationSpec::default()
        },
        labels: LabelsSpec {
            zh_cn: field.name.clone(),
            en: field.name.clone(),
        },
        widget: if hidden {
            WidgetSpec::Hidden
        } else {
            match value_type {
                ValueType::I8
                | ValueType::I16
                | ValueType::I32
                | ValueType::U8
                | ValueType::U16
                | ValueType::U32 => WidgetSpec::Number,
                ValueType::I64 | ValueType::U64 | ValueType::Decimal => WidgetSpec::Text,
                ValueType::Bool => WidgetSpec::Switch,
                ValueType::Date => WidgetSpec::Date,
                ValueType::DateTime => WidgetSpec::DateTime,
                ValueType::Json | ValueType::String => WidgetSpec::Text,
            }
        },
        enum_values: BTreeMap::new(),
    })
}

fn value_type(rust_type: &str) -> Result<(ValueType, bool), ResourceError> {
    let compact = rust_type
        .chars()
        .filter(|char| !char.is_whitespace())
        .collect::<String>();
    let (inner, nullable) = compact
        .strip_prefix("Option<")
        .and_then(|value| value.strip_suffix('>'))
        .map_or((compact.as_str(), false), |value| (value, true));
    let inner = inner
        .replace("ryframe_sdk::chrono::", "chrono::")
        .replace("ryframe_sdk::rust_decimal::", "rust_decimal::")
        .replace("ryframe_sdk::serde_json::", "serde_json::");
    let value = match inner.as_str() {
        "String" | "std::string::String" => ValueType::String,
        "i8" => ValueType::I8,
        "i16" => ValueType::I16,
        "i32" => ValueType::I32,
        "i64" => ValueType::I64,
        "u8" => ValueType::U8,
        "u16" => ValueType::U16,
        "u32" => ValueType::U32,
        "u64" => ValueType::U64,
        "rust_decimal::Decimal" => ValueType::Decimal,
        "bool" => ValueType::Bool,
        "chrono::NaiveDate" => ValueType::Date,
        "chrono::DateTime<chrono::Utc>" => ValueType::DateTime,
        "serde_json::Value" => ValueType::Json,
        _ => {
            return Err(error(format!(
                "暂不支持 ResourceModel 字段类型 `{rust_type}`"
            )));
        }
    };
    Ok((value, nullable))
}

fn default_value(value: &str, value_type: ValueType) -> Result<toml::Value, ResourceError> {
    match value_type {
        ValueType::String => Ok(toml::Value::String(value.to_owned())),
        ValueType::Decimal => Ok(toml::Value::String(value.to_owned())),
        ValueType::U64 => {
            let parsed = value
                .parse::<u64>()
                .map_err(|parse| error(format!("u64 默认值 `{value}` 无效：{parse}")))?;
            if parsed.to_string() != value {
                return Err(error(format!("u64 默认值 `{value}` 不是规范十进制整数")));
            }
            Ok(toml::Value::String(value.to_owned()))
        }
        ValueType::I8
        | ValueType::I16
        | ValueType::I32
        | ValueType::I64
        | ValueType::U8
        | ValueType::U16
        | ValueType::U32 => value
            .parse::<i64>()
            .map(toml::Value::Integer)
            .map_err(|parse| error(format!("整数默认值 `{value}` 无效：{parse}"))),
        ValueType::Bool => match value {
            "0" | "false" => Ok(toml::Value::Boolean(false)),
            "1" | "true" => Ok(toml::Value::Boolean(true)),
            _ => Err(error(format!(
                "布尔默认值 `{value}` 无效；使用 true/false 或数据库字面量 1/0"
            ))),
        },
        _ => Err(error("日期、JSON 与 decimal 字段不能声明自动 SQL 默认值")),
    }
}

fn indexes(descriptor: &OwnedResourceDescriptor) -> Vec<IndexSpec> {
    let mut indexes = descriptor
        .indexes
        .iter()
        .map(|index| IndexSpec {
            name: index.name.clone(),
            fields: index.fields.clone(),
            unique: index.unique,
        })
        .collect::<Vec<_>>();
    for field in &descriptor.fields {
        let fields = if field.unique
            && !field.auto_increment
            && descriptor.database == OwnedResourceDatabase::Tenant
            && field.name != "tenant_id"
        {
            vec!["tenant_id".into(), field.name.clone()]
        } else {
            vec![field.name.clone()]
        };
        let unique = field.unique && !field.auto_increment;
        let needed =
            unique || (field.auto_increment && descriptor.primary_key.first() != Some(&field.name));
        let covered = indexes.iter().any(|index| {
            if unique {
                index.unique && index.fields == fields
            } else {
                index.fields.first() == Some(&field.name)
            }
        });
        if needed && !covered {
            indexes.push(IndexSpec {
                name: format!(
                    "{}_{}_{}",
                    if unique { "uk" } else { "idx" },
                    descriptor.name,
                    field.name
                ),
                fields,
                unique,
            });
        }
    }
    indexes
}

fn soft_delete(
    descriptor: &OwnedResourceDescriptor,
) -> Result<Option<SoftDeleteSpec>, ResourceError> {
    let Some(field) = descriptor
        .fields
        .iter()
        .find(|field| field.name == "del_flag")
    else {
        return Ok(None);
    };
    let (value_type, _) = value_type(&field.rust_type)?;
    let (active, deleted) = match value_type {
        ValueType::String => (
            toml::Value::String("0".into()),
            toml::Value::String("1".into()),
        ),
        ValueType::I8
        | ValueType::I16
        | ValueType::I32
        | ValueType::I64
        | ValueType::U8
        | ValueType::U16
        | ValueType::U32 => (toml::Value::Integer(0), toml::Value::Integer(1)),
        ValueType::U64 => (
            toml::Value::String("0".into()),
            toml::Value::String("1".into()),
        ),
        ValueType::Bool => (toml::Value::Boolean(false), toml::Value::Boolean(true)),
        _ => return Err(error("del_flag 只支持 String、整数或 bool")),
    };
    Ok(Some(SoftDeleteSpec {
        field: field.name.clone(),
        active,
        deleted,
    }))
}

fn audit(descriptor: &OwnedResourceDescriptor) -> Option<AuditSpec> {
    let names = descriptor
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<BTreeSet<_>>();
    (names.contains("created_at") && names.contains("updated_at")).then(|| AuditSpec {
        created_at: "created_at".into(),
        created_by: names.contains("created_by").then(|| "created_by".into()),
        updated_at: "updated_at".into(),
        updated_by: names.contains("updated_by").then(|| "updated_by".into()),
    })
}

fn error(message: impl Into<String>) -> ResourceError {
    ResourceError::new(message, "修正业务 ResourceModel 后重新运行 cargo generate")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package() -> BusinessPackage {
        BusinessPackage {
            name: "order-business".into(),
            module: "order".into(),
            root: std::path::PathBuf::from("order-business"),
            workspace_root: std::path::PathBuf::from("."),
        }
    }

    fn field(name: &str, rust_type: &str, primary_key: bool) -> OwnedFieldDescriptor {
        OwnedFieldDescriptor {
            name: name.into(),
            column: name.into(),
            rust_type: rust_type.into(),
            sql_type: None,
            sql_default: None,
            sql_on_update: None,
            sql_comment: None,
            collation: None,
            rename_from: None,
            default: None,
            primary_key,
            generated: false,
            auto_increment: false,
            read_only: false,
            filter: false,
            sort: false,
            unique: false,
        }
    }

    fn descriptor(
        database: OwnedResourceDatabase,
        owner_field: Option<&str>,
        owner_type: &str,
    ) -> OwnedResourceDescriptor {
        let (table, primary_key, fields) = match database {
            OwnedResourceDatabase::Tenant => (
                "biz_order",
                vec!["tenant_id".into(), "id".into()],
                vec![
                    field("tenant_id", "String", true),
                    field("id", "i64", true),
                    field("owner_id", owner_type, false),
                ],
            ),
            OwnedResourceDatabase::Control => (
                "biz_global_order",
                vec!["id".into()],
                vec![
                    field("id", "i64", true),
                    field("owner_id", owner_type, false),
                ],
            ),
        };
        OwnedResourceDescriptor {
            name: "order".into(),
            title: "订单".into(),
            table: table.into(),
            collation: None,
            database,
            model_module: "order_business::resources".into(),
            model_type: "Order".into(),
            route: None,
            frontend_page: None,
            owner_field: owner_field.map(str::to_owned),
            menu_parent: None,
            menu_order: 100,
            primary_key,
            indexes: Vec::new(),
            fields,
        }
    }

    #[test]
    fn owner_field_defaults_to_none_and_is_omitted_from_descriptor_json() {
        let descriptor = descriptor(OwnedResourceDatabase::Tenant, None, "i64");
        let resource = descriptor_to_ir(&package(), &descriptor).unwrap();
        assert!(resource.access.owner_field.is_none());
        let json = serde_json::to_value(&descriptor).unwrap();
        assert!(json.get("owner_field").is_none());
    }

    #[test]
    fn declared_owner_field_flows_to_tenant_and_control_resources() {
        for database in [
            OwnedResourceDatabase::Tenant,
            OwnedResourceDatabase::Control,
        ] {
            for owner_type in ["i64", "Option<i64>"] {
                let descriptor = descriptor(database, Some("owner_id"), owner_type);
                let resource = descriptor_to_ir(&package(), &descriptor).unwrap();
                assert_eq!(resource.access.owner_field.as_deref(), Some("owner_id"));
                let owner = resource
                    .fields
                    .iter()
                    .find(|field| field.name == "owner_id")
                    .unwrap();
                assert!(!owner.usage.create && !owner.usage.create_optional && !owner.usage.update);
                assert!(matches!(owner.widget, crate::resource::WidgetIr::Hidden));
                assert!(owner.usage.read && owner.usage.list);
                assert_eq!(
                    resource.access.system_tenant_only,
                    database == OwnedResourceDatabase::Control
                );
            }
        }
    }

    #[test]
    fn owner_field_rejects_empty_missing_and_non_i64_fields() {
        for (owner_field, owner_type, message) in [
            (Some(""), "i64", "owner_field 不能为空"),
            (Some(" owner_id"), "i64", "含有首尾空白"),
            (Some("missing"), "i64", "未在模型中声明"),
            (Some("owner_id"), "String", "必须是 i64 或 Option<i64> 字段"),
        ] {
            let error = descriptor_to_ir(
                &package(),
                &descriptor(OwnedResourceDatabase::Tenant, owner_field, owner_type),
            )
            .unwrap_err();
            assert!(
                error.to_string().contains(message),
                "{owner_field:?} {owner_type}: {error}"
            );
        }
    }
}
