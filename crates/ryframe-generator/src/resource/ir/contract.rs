use std::collections::BTreeSet;

use super::{
    FieldIr, ResourceError, ResourceSpec, StorageKind, ValueType, ensure_field_reference,
    field_error,
};

pub(super) fn validate_storage(
    resource: &str,
    source_path: &str,
    spec: &ResourceSpec,
    fields: &BTreeSet<String>,
) -> Result<(), ResourceError> {
    match spec.storage.kind {
        StorageKind::TenantData => {
            let tenant_field = spec.storage.tenant_field.as_deref().ok_or_else(|| {
                ResourceError::new(
                    "tenant_data 缺少 tenant_field",
                    "将租户字段（通常为 tenant_id）写入 storage.tenant_field",
                )
                .with_resource(resource)
                .with_file(source_path)
            })?;
            ensure_field_reference(resource, source_path, fields, tenant_field, "租户隔离")?;
            if spec.database.primary_key.first().map(String::as_str) != Some(tenant_field) {
                return Err(field_error(
                    resource,
                    tenant_field,
                    source_path,
                    "tenant_data 的主键没有以租户字段开头",
                    "把 tenant_field 放到 database.primary_key 第一位",
                ));
            }
            for index in spec.database.indexes.iter().filter(|index| index.unique) {
                if !index.fields.iter().any(|field| field == tenant_field) {
                    return Err(ResourceError::new(
                        format!("唯一索引 `{}` 未包含租户字段", index.name),
                        format!("将 `{tenant_field}` 加入该唯一索引"),
                    )
                    .with_resource(resource)
                    .with_file(source_path));
                }
            }
        }
        StorageKind::ControlRow => {
            if spec.storage.tenant_field.is_some() {
                return Err(ResourceError::new(
                    "control_row 不应声明 tenant_field",
                    "删除 storage.tenant_field；租户数据请改用 tenant_data",
                )
                .with_resource(resource)
                .with_file(source_path));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_generation_contract(
    resource: &str,
    source_path: &str,
    spec: &ResourceSpec,
    fields: &[FieldIr],
) -> Result<(), ResourceError> {
    let error = |message: &str, suggestion: &str| {
        ResourceError::new(message, suggestion)
            .with_resource(resource)
            .with_file(source_path)
    };
    if spec.resource.module != "system" {
        return Err(error(
            "flat_crud v1 仅支持 system 模块",
            "其他模块使用普通手写切片；扩展生成边界时同时补真实 Workspace 编译测试",
        ));
    }
    if !spec.api.path.starts_with("/api/v1/system/")
        || spec.api.path.ends_with('/')
        || spec.api.path.contains("//")
        || spec.api.path.contains('{')
        || spec.api.path.contains('}')
    {
        return Err(error(
            "flat_crud v1 API 路径必须是无占位符的 /api/v1/system/<resources>",
            "使用例如 `/api/v1/system/posts` 的集合路径，不要添加尾斜杠或路径参数",
        ));
    }

    let id = fields
        .iter()
        .find(|field| field.name == "id")
        .ok_or_else(|| error("flat_crud 缺少 id 字段", "声明非空 value_type=i64 的 id"))?;
    if id.value_type != ValueType::I64 || id.nullable {
        return Err(field_error(
            resource,
            "id",
            source_path,
            "flat_crud id 必须是非空 i64",
            "将 id 设置为 value_type=i64、nullable=false",
        ));
    }
    let expected_key: &[&str] = match spec.storage.kind {
        StorageKind::ControlRow => &["id"],
        StorageKind::TenantData => &["tenant_id", "id"],
    };
    if spec
        .database
        .primary_key
        .iter()
        .map(String::as_str)
        .ne(expected_key.iter().copied())
    {
        return Err(error(
            "flat_crud 主键超出 v1 安全生成范围",
            "control_row 使用 [id]；tenant_data 使用 [tenant_id, id]",
        ));
    }
    if spec.storage.kind == StorageKind::TenantData
        && spec.storage.tenant_field.as_deref() != Some("tenant_id")
    {
        return Err(error(
            "tenant_data 的 tenant_field 必须是 tenant_id",
            "设置 storage.tenant_field = \"tenant_id\" 并将其放在复合主键第一位",
        ));
    }
    let tenant = fields
        .iter()
        .find(|field| field.name == "tenant_id")
        .ok_or_else(|| {
            error(
                "flat_crud 缺少 tenant_id 字段",
                "声明非空 string tenant_id；该字段由服务管理",
            )
        })?;
    if tenant.value_type != ValueType::String || tenant.nullable {
        return Err(field_error(
            resource,
            "tenant_id",
            source_path,
            "tenant_id 必须是非空 string",
            "将 tenant_id 设置为 value_type=string、nullable=false",
        ));
    }
    for index in spec.database.indexes.iter().filter(|index| index.unique) {
        if !index.fields.iter().any(|field| field == "tenant_id") {
            return Err(error(
                &format!("唯一业务索引 `{}` 未包含 tenant_id", index.name),
                "把 tenant_id 加入唯一索引，避免跨租户冲突",
            ));
        }
        if index.fields.iter().all(|field| field == "tenant_id") {
            return Err(error(
                &format!("唯一索引 `{}` 没有业务字段", index.name),
                "唯一业务索引除 tenant_id 外至少声明一个业务字段",
            ));
        }
    }

    let audit = spec.database.audit.as_ref().ok_or_else(|| {
        error(
            "flat_crud v1 必须声明 created_at/updated_at",
            "在 database.audit 中声明两个 date_time 字段",
        )
    })?;
    if audit.created_by.is_some() || audit.updated_by.is_some() {
        return Err(error(
            "flat_crud v1 尚未实现 created_by/updated_by",
            "把操作者主体规则放入强类型 Service 扩展，不要让生成器静默忽略",
        ));
    }
    for audit_field in [&audit.created_at, &audit.updated_at] {
        let field = fields
            .iter()
            .find(|field| field.name == *audit_field)
            .expect("审计字段引用已校验");
        if field.value_type != ValueType::DateTime {
            return Err(field_error(
                resource,
                audit_field,
                source_path,
                "审计时间字段必须是 date_time",
                "将 created_at/updated_at 的 value_type 改为 date_time",
            ));
        }
    }

    let mut service_managed = BTreeSet::from([
        "id".to_owned(),
        "tenant_id".to_owned(),
        audit.created_at.clone(),
        audit.updated_at.clone(),
    ]);
    if let Some(soft_delete) = &spec.database.soft_delete {
        service_managed.insert(soft_delete.field.clone());
    }
    for field in fields {
        let editable = field.usage.create || field.usage.update;
        if service_managed.contains(&field.name) && editable {
            return Err(field_error(
                resource,
                &field.name,
                source_path,
                "服务管理字段禁止 create/update",
                "关闭 usage.create/update，由 Service 写入租户、主键、软删和审计值",
            ));
        }
        if editable && !(field.usage.read || field.usage.list) {
            return Err(field_error(
                resource,
                &field.name,
                source_path,
                "表单字段未出现在 read/list 视图",
                "至少启用 usage.read 或 usage.list，确保编辑表单取得强类型原值",
            ));
        }
        if !field.nullable && field.usage.create_optional && field.default.is_none() {
            return Err(field_error(
                resource,
                &field.name,
                source_path,
                "非空 create_optional 字段缺少默认值",
                "声明类型正确且满足校验的 default，或取消 create_optional",
            ));
        }
        if !field.nullable
            && !field.usage.create
            && !service_managed.contains(&field.name)
            && field.default.is_none()
        {
            return Err(field_error(
                resource,
                &field.name,
                source_path,
                "非空字段既不从 create 输入，也没有稳定默认值",
                "启用 usage.create、声明类型正确的 default，或移入强类型 Service 扩展",
            ));
        }
    }
    let default_sort = fields
        .iter()
        .filter(|field| field.usage.sort)
        .collect::<Vec<_>>();
    if default_sort.len() > 1 {
        let names = default_sort
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>()
            .join("、");
        return Err(field_error(
            resource,
            &default_sort[1].name,
            source_path,
            format!("flat_crud v1 只能声明一个默认排序字段，当前为 {names}"),
            "只为一个字段保留 usage.sort=true；稳定次级 id 排序由 repository 自动追加",
        ));
    }
    Ok(())
}
