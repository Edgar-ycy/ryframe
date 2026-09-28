use std::collections::BTreeSet;

use super::{
    AccessIr, ApiIr, AuditIr, FieldIr, FieldSpec, FieldUsageIr, IndexIr, LabelsIr, LabelsSpec,
    MenuIr, PermissionIr, RelationIr, ResourceError, ResourceIr, ResourceSpec, RouteIr,
    SoftDeleteIr, ValidationIr, WidgetIr, field_error, is_snake_identifier, normalize_schema,
    validate_api_and_access, validate_extensions, validate_field, validate_generation_contract,
    validate_labels, validate_references, validate_storage,
};
use super::{OperationSpec, PermissionSpec, WidgetSpec};

pub(in crate::resource) fn normalize(
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

    let (names, fields) = normalize_fields(&mut spec, &resource, &source_path)?;

    let mut relations = spec
        .relations
        .iter()
        .map(|relation| RelationIr {
            name: relation.name.clone(),
            pascal_name: crate::naming::to_pascal_case(&relation.name),
            kind: relation.kind,
            local_field: relation.local_field.clone(),
            target_resource: relation.target_resource.clone(),
            target_pascal_name: crate::naming::to_pascal_case(&relation.target_resource),
        })
        .collect::<Vec<_>>();
    relations.sort_by(|left, right| left.name.cmp(&right.name));

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
        configuration_versioned: spec.storage.configuration_versioned,
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
        relations,
        api: ApiIr {
            path: spec.api.path,
            operations: operations(spec.api.operations),
        },
        access: AccessIr {
            capability: spec.access.capability,
            owner_field: spec.access.owner_field,
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

pub(super) fn operations(value: OperationSpec) -> PermissionIr {
    PermissionIr {
        create: value.create,
        read: value.read,
        list: value.list,
        update: value.update,
        delete: value.delete,
    }
}

pub(super) fn permissions(value: PermissionSpec) -> PermissionIr {
    PermissionIr {
        create: value.create,
        read: value.read,
        list: value.list,
        update: value.update,
        delete: value.delete,
    }
}

fn normalize_fields(
    spec: &mut ResourceSpec,
    resource: &str,
    source_path: &str,
) -> Result<(BTreeSet<String>, Vec<FieldIr>), ResourceError> {
    let mut names = BTreeSet::new();
    let mut columns = BTreeSet::new();
    let mut orders = BTreeSet::new();
    let input_fields = std::mem::take(&mut spec.fields);
    let mut fields = Vec::with_capacity(input_fields.len());
    for field in input_fields {
        validate_field(resource, source_path, &field)?;
        if !names.insert(field.name.clone()) {
            return Err(field_error(
                resource,
                &field.name,
                source_path,
                "字段名重复",
                "删除重复字段或为字段改用唯一名称",
            ));
        }
        let column = field.column.clone().unwrap_or_else(|| field.name.clone());
        if !columns.insert(column.clone()) {
            return Err(field_error(
                resource,
                &field.name,
                source_path,
                format!("数据库列名 `{column}` 重复"),
                "为每个字段配置唯一的 column，或删除不必要的别名",
            ));
        }
        if !orders.insert(field.order) {
            return Err(field_error(
                resource,
                &field.name,
                source_path,
                format!("字段顺序 {} 重复", field.order),
                "为每个字段分配唯一 order",
            ));
        }
        fields.push(normalize_field(field, column));
    }
    fields.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then(left.name.cmp(&right.name))
    });

    Ok((names, fields))
}

fn normalize_field(field: FieldSpec, column: String) -> FieldIr {
    let nullable = field.nullable;
    let wire_type = field.wire_type.unwrap_or(field.value_type);
    FieldIr {
        name: field.name,
        column,
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
            filter_exact: field.usage.filter_exact,
            sort: field.usage.sort,
            sort_desc: field.usage.sort_desc,
        },
        validation: ValidationIr {
            required: field.validation.required,
            min_length: field.validation.min_length,
            max_length: field.validation.max_length,
            min_utf8_bytes: field.validation.min_utf8_bytes,
            max_utf8_bytes: field.validation.max_utf8_bytes,
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
    }
}
