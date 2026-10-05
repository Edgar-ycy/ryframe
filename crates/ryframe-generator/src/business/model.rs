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
    model: Option<&str>,
) -> Result<BTreeSet<String>, ResourceError> {
    match model {
        None => Ok(descriptors.iter().map(|item| item.name.clone()).collect()),
        Some(model) => {
            let selected = descriptors
                .iter()
                .find(|item| {
                    item.name.eq_ignore_ascii_case(model)
                        || item.model_type.eq_ignore_ascii_case(model)
                })
                .ok_or_else(|| error(format!("业务模型 `{model}` 不存在")))?;
            Ok(BTreeSet::from([selected.name.clone()]))
        }
    }
}

pub(super) fn descriptor_to_ir(
    package: &BusinessPackage,
    descriptor: &OwnedResourceDescriptor,
) -> Result<ResourceIr, ResourceError> {
    validate_descriptor(package, descriptor)?;
    let fields = descriptor
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| field_spec(field, index))
        .collect::<Result<Vec<_>, _>>()?;
    let primary_key = descriptor
        .fields
        .iter()
        .filter(|field| field.primary_key)
        .map(|field| field.name.clone())
        .collect::<Vec<_>>();
    let indexes = indexes(descriptor);
    let soft_delete = soft_delete(descriptor)?;
    let audit = audit(descriptor);
    let module = package.module.replace('-', "_");
    let route_key = format!("{module}.{}", descriptor.name);
    let api_name = format!("{}_{}", module, descriptor.name);
    let labels = LabelsSpec {
        zh_cn: descriptor.title.clone(),
        en: descriptor.model_type.clone(),
    };
    let mut frontend = BTreeMap::new();
    frontend.insert(
        "page".into(),
        toml::Value::String(format!(
            "@/views/business/{module}/{}/index.vue",
            descriptor.name
        )),
    );
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
            tenant_field: descriptor
                .fields
                .iter()
                .any(|field| field.name == "tenant_id")
                .then(|| "tenant_id".into()),
            configuration_versioned: false,
        },
        database: DatabaseSpec {
            table: descriptor.table.clone(),
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
            capability: None,
            owner_field: None,
            permissions: PermissionSpec {
                create: format!("{module}:{}:add", descriptor.name),
                read: format!("{module}:{}:list", descriptor.name),
                list: format!("{module}:{}:list", descriptor.name),
                update: format!("{module}:{}:edit", descriptor.name),
                delete: format!("{module}:{}:remove", descriptor.name),
            },
        },
        menu: MenuSpec {
            key: route_key.clone(),
            parent: descriptor
                .menu_parent
                .clone()
                .unwrap_or_else(|| module.clone()),
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
        value_type,
        wire_type: (value_type == ValueType::I64).then_some(ValueType::String),
        order: u16::try_from((index + 1) * 10).map_err(|_| error("字段数量超过生成上限"))?,
        nullable,
        default: field
            .default
            .as_deref()
            .map(|value| default_value(value, value_type))
            .transpose()?,
        usage: FieldUsageSpec {
            create: mutable,
            create_optional: mutable && (nullable || field.default.is_some()),
            update: mutable,
            update_optional: mutable,
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
                ValueType::I32 | ValueType::I64 => WidgetSpec::Number,
                ValueType::Bool => WidgetSpec::Switch,
                ValueType::Date => WidgetSpec::Date,
                ValueType::DateTime => WidgetSpec::DateTime,
                ValueType::Json | ValueType::String => WidgetSpec::Text,
                ValueType::Decimal => unreachable!("decimal 类型已拒绝"),
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
        .replace("ryframe_sdk::serde_json::", "serde_json::");
    let value = match inner.as_str() {
        "String" | "std::string::String" => ValueType::String,
        "i32" => ValueType::I32,
        "i64" => ValueType::I64,
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
        ValueType::I32 | ValueType::I64 => value
            .parse::<i64>()
            .map(toml::Value::Integer)
            .map_err(|parse| error(format!("整数默认值 `{value}` 无效：{parse}"))),
        ValueType::Bool => value
            .parse::<bool>()
            .map(toml::Value::Boolean)
            .map_err(|parse| error(format!("布尔默认值 `{value}` 无效：{parse}"))),
        _ => Err(error("日期、JSON 与 decimal 字段不能声明自动 SQL 默认值")),
    }
}

fn indexes(descriptor: &OwnedResourceDescriptor) -> Vec<IndexSpec> {
    descriptor
        .fields
        .iter()
        .filter(|field| field.unique || field.name == "tenant_id")
        .map(|field| IndexSpec {
            name: format!(
                "{}_{}_{}",
                if field.unique { "uk" } else { "idx" },
                descriptor.name,
                field.name
            ),
            fields: if field.unique
                && descriptor.database == OwnedResourceDatabase::Tenant
                && field.name != "tenant_id"
            {
                vec!["tenant_id".into(), field.name.clone()]
            } else {
                vec![field.name.clone()]
            },
            unique: field.unique,
        })
        .collect()
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
        ValueType::I32 | ValueType::I64 => (toml::Value::Integer(0), toml::Value::Integer(1)),
        ValueType::Bool => (toml::Value::Boolean(false), toml::Value::Boolean(true)),
        _ => return Err(error("del_flag 只支持 String、i32、i64 或 bool")),
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
