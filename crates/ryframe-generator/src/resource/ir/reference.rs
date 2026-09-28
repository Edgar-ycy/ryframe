use std::collections::BTreeSet;

use super::{
    FieldIr, ResourceError, ResourceSpec, ValueType, field_error, is_safe_symbol,
    is_snake_identifier, validate_ir_value,
};

pub(super) fn validate_references(
    resource: &str,
    source_path: &str,
    spec: &ResourceSpec,
    fields: &BTreeSet<String>,
    field_specs: &[FieldIr],
) -> Result<(), ResourceError> {
    validate_keys(resource, source_path, spec, fields)?;
    validate_relations(resource, source_path, spec, fields, field_specs)?;
    validate_lifecycle_references(resource, source_path, spec, fields, field_specs)?;
    Ok(())
}

pub(super) fn ensure_field_reference(
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

fn validate_keys(
    resource: &str,
    source_path: &str,
    spec: &ResourceSpec,
    fields: &BTreeSet<String>,
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
    Ok(())
}

fn validate_relations(
    resource: &str,
    source_path: &str,
    spec: &ResourceSpec,
    fields: &BTreeSet<String>,
    field_specs: &[FieldIr],
) -> Result<(), ResourceError> {
    let mut relation_names = BTreeSet::new();
    for relation in &spec.relations {
        if !is_snake_identifier(&relation.name) {
            return Err(ResourceError::new(
                format!("关系名 `{}` 不是安全标识符", relation.name),
                "关系名只使用小写字母、数字和下划线",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
        if !relation_names.insert(&relation.name) {
            return Err(ResourceError::new(
                format!("关系 `{}` 重复", relation.name),
                "每个详情关系使用唯一名称",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
        if fields.contains(&relation.name) {
            return Err(field_error(
                resource,
                &relation.name,
                source_path,
                "关系名与资源字段重名",
                "为关系使用不会覆盖详情字段的名称",
            ));
        }
        ensure_field_reference(resource, source_path, fields, &relation.local_field, "关系")?;
        let local_field = field_specs
            .iter()
            .find(|field| field.name == relation.local_field)
            .expect("关系字段引用已校验");
        if local_field.value_type != ValueType::I64 {
            return Err(field_error(
                resource,
                &relation.local_field,
                source_path,
                "belongs_to 关系字段必须是 i64",
                "将外键字段设为 i64；通过 wire_type=string 保持前端 ID 精度",
            ));
        }
        if !is_snake_identifier(&relation.target_resource) {
            return Err(ResourceError::new(
                format!("关系目标 `{}` 不是安全资源名", relation.target_resource),
                "target_resource 只使用已声明资源的小写 snake_case 名称",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
    }
    Ok(())
}

fn validate_lifecycle_references(
    resource: &str,
    source_path: &str,
    spec: &ResourceSpec,
    fields: &BTreeSet<String>,
    field_specs: &[FieldIr],
) -> Result<(), ResourceError> {
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
    if let Some(owner_field) = &spec.access.owner_field {
        ensure_field_reference(resource, source_path, fields, owner_field, "数据范围")?;
        let field = field_specs
            .iter()
            .find(|field| field.name == *owner_field)
            .expect("数据范围字段引用已校验");
        if field.value_type != ValueType::I64 {
            return Err(field_error(
                resource,
                owner_field,
                source_path,
                "owner_field 必须是 i64 字段",
                "使用保存用户 ID 的 i64 字段作为 access.owner_field",
            ));
        }
    }
    Ok(())
}
