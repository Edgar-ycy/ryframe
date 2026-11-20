use super::super::{FieldIr, ResourceIr, StorageKind, rust_literal};
use super::{
    argument_expression, business_index_fields, unique_business_indexes, unique_method_name,
};

pub(crate) fn service(resource: &ResourceIr, header: &str) -> String {
    let scope_argument = if resource.access.owner_field.is_some() {
        ", &actor.data_scope_context()"
    } else {
        ""
    };
    let id_type = super::super::id_type(resource);
    let pascal = &resource.pascal_name;
    let insert_type = super::super::insert_type(resource);
    let insert_import = super::super::insert_import(resource);
    let not_found = format!("{}不存在", resource.labels.zh_cn);
    let audit_mode = match resource.storage {
        StorageKind::ControlRow => "TransactionAuditMode::CurrentRequest",
        StorageKind::TenantData => "TransactionAuditMode::Skip",
    };
    let record_fields = resource
        .fields
        .iter()
        .filter(|field| !field.database_default())
        .map(|field| {
            format!(
                "                {}: {},",
                field.name,
                create_expression(resource, field)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let updates = update_fields(resource);
    let update_revision = resource
        .revision_field
        .as_deref()
        .map(|field| {
            format!(
                "            if record.{field} != command.expected_revision {{\n                return Err(AppError::Conflict(\"记录版本已变化，请重新读取后更新\".into()));\n            }}\n            record.{field} = record.{field}.checked_add(1).ok_or_else(|| AppError::Conflict(\"记录版本已达上限\".into()))?;\n"
            )
        })
        .unwrap_or_default();
    let filter_fields = resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && field.name != "tenant_id")
        .map(|field| {
            if matches!(field.value_type, super::super::ValueType::String) {
                format!(
                    "            {}: params.{}.as_deref(),",
                    field.name, field.name
                )
            } else {
                format!("            {}: params.{},", field.name, field.name)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let filter_fields = if resource.access.owner_field.is_some() {
        [
            "            data_scope: &data_scope,".to_owned(),
            filter_fields,
        ]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
    } else {
        filter_fields
    };
    let data_scope = resource
        .access
        .owner_field
        .as_ref()
        .map(|_| "        let data_scope = actor.data_scope_context();\n")
        .unwrap_or_default();
    let lock_data_scope =
        if resource.access.owner_field.is_some() && resource.storage == StorageKind::TenantData {
            "            transaction.lock_data_scope(&actor.data_scope_context()).await?;\n"
        } else {
            ""
        };
    let WriteStages {
        create_before,
        create_after,
        write_before,
        update_unique_checks,
        write_after,
    } = write_stages(resource);
    let updates = format!("{updates}\n{update_unique_checks}");
    let system_tenant_guard = super::super::system_tenant_guard(resource, "crate");
    let create_time = resource.audit.as_ref().is_some_and(|audit| {
        resource.fields.iter().any(|field| {
            (field.name == audit.created_at || field.name == audit.updated_at)
                && !field.database_clock()
        })
    });
    let update_time = resource.audit.as_ref().is_some_and(|audit| {
        resource
            .fields
            .iter()
            .any(|field| field.name == audit.updated_at && !field.database_update())
    });
    let time_import = if create_time || update_time {
        "use chrono::Utc;\n"
    } else {
        ""
    };
    let create_clock = if create_time {
        "        let now = Utc::now();\n"
    } else {
        ""
    };
    let update_clock = if update_time {
        "            let now = Utc::now();\n"
    } else {
        ""
    };
    let decimal_record = decimal_checks(resource, "record", false);
    let decimal_filter = decimal_checks(resource, "params", true);
    let output = format!(
        "{header}use std::sync::Arc;\n\n{time_import}use ryframe_kernel::{{ActorContext, AppError, AppResult, PageResult}};\n\nuse crate::{{TransactionAuditMode, complete_transaction}};\n\nuse super::model::{{Create{pascal}Command, {pascal}Filter, {pascal}ListParams, {pascal}Record{insert_import}, Update{pascal}Command}};\nuse super::port::{pascal}PersistencePort;\n\npub struct {pascal}Service {{\n    persistence: Arc<dyn {pascal}PersistencePort>,\n}}\n\nimpl {pascal}Service {{\n    pub fn new(persistence: Arc<dyn {pascal}PersistencePort>) -> Self {{\n        Self {{ persistence }}\n    }}\n\n    pub async fn find_by_id(\n        &self,\n        actor: &ActorContext,\n        id: {id_type},\n    ) -> AppResult<Option<{pascal}Record>> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n{system_tenant_guard}        self.persistence.find_by_id(tenant_id, id{scope_argument}).await\n    }}\n\n    pub async fn create(\n        &self,\n        actor: &ActorContext,\n        command: Create{pascal}Command,\n    ) -> AppResult<{pascal}Record> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n{system_tenant_guard}{create_clock}        let record = {insert_type} {{\n{record_fields}\n        }};\n{decimal_record}        let transaction = self.persistence.begin(tenant_id).await?;\n        let operation = async {{\n{lock_data_scope}{create_before}\n            let saved = transaction.insert(record).await?;\n{create_after}\n            Ok(saved)\n        }}\n        .await;\n        complete_transaction(transaction, operation, {audit_mode}).await\n    }}\n\n    pub async fn update(\n        &self,\n        actor: &ActorContext,\n        id: {id_type},\n        command: Update{pascal}Command,\n    ) -> AppResult<{pascal}Record> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n{system_tenant_guard}        let transaction = self.persistence.begin(tenant_id).await?;\n        let operation = async {{\n{write_before}\n            let mut record = transaction\n                .find_by_id_for_update(tenant_id, id{scope_argument})\n                .await?\n                .ok_or_else(|| AppError::NotFound({not_found:?}.into()))?;\n{update_revision}{update_clock}{updates}\n{decimal_record}            let saved = transaction.update(record).await?;\n{write_after}\n            Ok(saved)\n        }}\n        .await;\n        complete_transaction(transaction, operation, {audit_mode}).await\n    }}\n\n    pub async fn delete(&self, actor: &ActorContext, id: {id_type}) -> AppResult<()> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n{system_tenant_guard}        let transaction = self.persistence.begin(tenant_id).await?;\n        let operation = async {{\n{write_before}\n            transaction\n                .find_by_id_for_update(tenant_id, id{scope_argument})\n                .await?\n                .ok_or_else(|| AppError::NotFound({not_found:?}.into()))?;\n            transaction.delete(tenant_id, id).await?;\n{write_after}\n            Ok(())\n        }}\n        .await;\n        complete_transaction(transaction, operation, {audit_mode}).await\n    }}\n\n    pub async fn find_by_page(\n        &self,\n        actor: &ActorContext,\n        params: {pascal}ListParams,\n    ) -> AppResult<PageResult<{pascal}Record>> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n{system_tenant_guard}{decimal_filter}{data_scope}        let filter = {pascal}Filter {{\n{filter_fields}\n        }};\n        self.persistence\n            .find_by_page(tenant_id, params.page, filter)\n            .await\n    }}\n}}\n",
    );
    if resource.relations.is_empty() {
        output
    } else {
        output.replace(
            &format!("AppResult<Option<{pascal}Record>>"),
            &format!("AppResult<Option<super::model::{pascal}Detail>>"),
        )
    }
}
fn create_expression(resource: &ResourceIr, field: &FieldIr) -> String {
    if field.name == "id" {
        if resource.id_generation == crate::IdGeneration::AutoIncrement {
            return "0".into();
        }
        return if field.value_type == super::super::ValueType::U64 {
            "u64::try_from(crate::next_id()?).map_err(|_| AppError::Internal(\"雪花 ID 不能转换为 u64\".into()))?".into()
        } else {
            "crate::next_id()?".into()
        };
    }
    if field.name == "tenant_id" {
        return "tenant_id.to_owned()".into();
    }
    if resource.access.owner_field.as_deref() == Some(field.name.as_str()) {
        return if field.nullable {
            "Some(actor.user_id)".into()
        } else {
            "actor.user_id".into()
        };
    }
    if let Some(audit) = &resource.audit
        && (audit.created_at == field.name || audit.updated_at == field.name)
        && !field.database_clock()
    {
        return if field.nullable {
            "Some(now)".into()
        } else {
            "now".into()
        };
    }
    if let Some(audit) = &resource.audit
        && (audit.created_by.as_deref() == Some(&field.name)
            || audit.updated_by.as_deref() == Some(&field.name))
    {
        return if field.nullable {
            "Some(actor.user_id)".into()
        } else {
            "actor.user_id".into()
        };
    }
    if let Some(soft_delete) = &resource.soft_delete
        && soft_delete.field == field.name
    {
        return rust_literal(&soft_delete.active, field.value_type);
    }
    if field.usage.create {
        if field.usage.create_optional {
            let fallback = field
                .default
                .as_ref()
                .map(|value| rust_literal(value, field.value_type))
                .expect("非空 create_optional 默认值已由 IR 校验");
            return if field.value_type == super::super::ValueType::String {
                format!("command.{}.unwrap_or_else(|| {fallback})", field.name)
            } else {
                format!("command.{}.unwrap_or({fallback})", field.name)
            };
        }
        return format!("command.{}", field.name);
    }
    if let Some(default) = &field.default {
        let value = rust_literal(default, field.value_type);
        return if field.nullable {
            format!("Some({value})")
        } else {
            value
        };
    }
    if field.nullable {
        return "None".into();
    }
    unreachable!("非空字段的创建来源已由 Resource IR 校验")
}

fn update_fields(resource: &ResourceIr) -> String {
    resource
        .fields
        .iter()
        .filter(|field| {
            field.usage.update
                && resource.access.owner_field.as_deref() != Some(field.name.as_str())
        })
        .map(|field| {
            if field.usage.update_optional {
                format!(
                    "            if let Some(value) = command.{0} {{\n                record.{0} = value;\n            }}",
                    field.name
                )
            } else {
                format!("            record.{0} = command.{0};", field.name)
            }
        })
        .chain(resource.audit.iter().filter(|audit| {
            !resource.fields.iter().any(|field| field.name == audit.updated_at && field.database_update())
        }).map(|audit| {
            let updated = resource
                .fields
                .iter()
                .find(|field| field.name == audit.updated_at)
                .expect("审计字段已在 IR 校验");
            if updated.nullable {
                format!("            record.{} = Some(now);", audit.updated_at)
            } else {
                format!("            record.{} = now;", audit.updated_at)
            }
        }))
        .chain(resource.audit.iter().filter_map(|audit| {
            audit.updated_by.as_ref().map(|field| {
                let updated = resource
                    .fields
                    .iter()
                    .find(|candidate| candidate.name == *field)
                    .expect("操作者审计字段已在 IR 校验");
                if updated.nullable {
                    format!("            record.{field} = Some(actor.user_id);")
                } else {
                    format!("            record.{field} = actor.user_id;")
                }
            })
        }))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Default)]
struct WriteStages {
    create_before: String,
    create_after: String,
    write_before: String,
    update_unique_checks: String,
    write_after: String,
}

fn write_stages(resource: &ResourceIr) -> WriteStages {
    if resource.storage != StorageKind::ControlRow {
        return WriteStages::default();
    }
    let (lock, increment) = if resource.configuration_versioned {
        (
            "            transaction.lock_configuration(tenant_id).await?;",
            "            transaction.increment_configuration_version(tenant_id).await?;",
        )
    } else {
        ("", "")
    };
    WriteStages {
        create_before: [lock, &unique_checks(resource, false)]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        create_after: increment.into(),
        write_before: lock.into(),
        update_unique_checks: unique_checks(resource, true),
        write_after: increment.into(),
    }
}

fn unique_checks(resource: &ResourceIr, update: bool) -> String {
    unique_business_indexes(resource)
        .filter(|index| {
            let fields = business_index_fields(resource, index);
            (!update || fields.iter().any(|field| field.usage.update))
                && !fields.iter().any(|field| field.database_default()
                    || (update && field.database_update() && field.sql_on_update.is_some()))
        })
        .map(|index| {
            let fields = business_index_fields(resource, index);
            let arguments = fields.iter().map(|field| argument_expression(field, &format!("record.{}", field.name)))
                .collect::<Vec<_>>().join(", ");
            let label = fields.iter().map(|field| field.labels.zh_cn.as_str()).collect::<Vec<_>>().join("、");
            let exclude = if update { "Some(id)" } else { "None" };
            format!(
                "            if transaction.{method}(tenant_id, {arguments}, {exclude}).await?.is_some() {{\n                return Err(AppError::Conflict({message:?}.into()));\n            }}",
                method = unique_method_name(index), message = format!("{label}已存在"),
            )
        })
        .collect::<Vec<_>>().join("\n")
}

fn decimal_checks(resource: &ResourceIr, owner: &str, filter: bool) -> String {
    resource.fields.iter().filter(|field| field.value_type == super::super::ValueType::Decimal && (!filter || field.usage.filter)).map(|field| {
        let (precision, scale) = crate::resource::decimal::capacity(field.sql_type.as_deref().expect("Decimal 类型已校验")).expect("Decimal 容量已校验");
        let check = if field.nullable || filter {
            format!("{owner}.{0}.is_some_and(|value| !ryframe_kernel::decimal_fits_precision(value, {precision}, {scale}))", field.name)
        } else {
            format!("!ryframe_kernel::decimal_fits_precision({owner}.{0}, {precision}, {scale})", field.name)
        };
        let message = format!("{} 必须符合 DECIMAL({precision},{scale})，不能舍入或溢出", field.labels.zh_cn);
        format!("        if {check} {{\n            return Err(AppError::Validation({message:?}.into()));\n        }}\n")
    }).collect()
}
