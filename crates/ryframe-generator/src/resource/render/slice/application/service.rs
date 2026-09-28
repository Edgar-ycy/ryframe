use super::super::{FieldIr, ResourceIr, StorageKind, rust_literal};
use super::{
    argument_expression, business_index_fields, unique_business_indexes, unique_method_name,
};

pub(crate) fn service(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let not_found = format!("{}不存在", resource.labels.zh_cn);
    let audit_mode = match resource.storage {
        StorageKind::ControlRow => "TransactionAuditMode::CurrentRequest",
        StorageKind::TenantData => "TransactionAuditMode::Skip",
    };
    let record_fields = resource
        .fields
        .iter()
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
    let WriteStages {
        create_before,
        create_after,
        write_before,
        update_unique_checks,
        write_after,
    } = write_stages(resource);
    let updates = format!("{updates}\n{update_unique_checks}");
    let output = format!(
        "{header}use std::sync::Arc;\n\nuse chrono::Utc;\nuse ryframe_kernel::{{ActorContext, AppError, AppResult, PageResult}};\n\nuse crate::{{TransactionAuditMode, complete_transaction}};\n\nuse super::model::{{Create{pascal}Command, {pascal}Filter, {pascal}ListParams, {pascal}Record, Update{pascal}Command}};\nuse super::port::{pascal}PersistencePort;\n\npub struct {pascal}Service {{\n    persistence: Arc<dyn {pascal}PersistencePort>,\n}}\n\nimpl {pascal}Service {{\n    pub fn new(persistence: Arc<dyn {pascal}PersistencePort>) -> Self {{\n        Self {{ persistence }}\n    }}\n\n    pub async fn find_by_id(\n        &self,\n        actor: &ActorContext,\n        id: i64,\n    ) -> AppResult<Option<{pascal}Record>> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n        self.persistence.find_by_id(tenant_id, id).await\n    }}\n\n    pub async fn create(\n        &self,\n        actor: &ActorContext,\n        command: Create{pascal}Command,\n    ) -> AppResult<{pascal}Record> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n        let now = Utc::now();\n        let record = {pascal}Record {{\n{record_fields}\n        }};\n        let transaction = self.persistence.begin(tenant_id).await?;\n        let operation = async {{\n{create_before}\n            let saved = transaction.insert(record).await?;\n{create_after}\n            Ok(saved)\n        }}\n        .await;\n        complete_transaction(transaction, operation, {audit_mode}).await\n    }}\n\n    pub async fn update(\n        &self,\n        actor: &ActorContext,\n        id: i64,\n        command: Update{pascal}Command,\n    ) -> AppResult<{pascal}Record> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n        let transaction = self.persistence.begin(tenant_id).await?;\n        let operation = async {{\n{write_before}\n            let mut record = transaction\n                .find_by_id_for_update(tenant_id, id)\n                .await?\n                .ok_or_else(|| AppError::NotFound({not_found:?}.into()))?;\n            let now = Utc::now();\n{updates}\n            let saved = transaction.update(record).await?;\n{write_after}\n            Ok(saved)\n        }}\n        .await;\n        complete_transaction(transaction, operation, {audit_mode}).await\n    }}\n\n    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n        let transaction = self.persistence.begin(tenant_id).await?;\n        let operation = async {{\n{write_before}\n            transaction\n                .find_by_id_for_update(tenant_id, id)\n                .await?\n                .ok_or_else(|| AppError::NotFound({not_found:?}.into()))?;\n            transaction.delete(tenant_id, id).await?;\n{write_after}\n            Ok(())\n        }}\n        .await;\n        complete_transaction(transaction, operation, {audit_mode}).await\n    }}\n\n    pub async fn find_by_page(\n        &self,\n        actor: &ActorContext,\n        params: {pascal}ListParams,\n    ) -> AppResult<PageResult<{pascal}Record>> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n{data_scope}        let filter = {pascal}Filter {{\n{filter_fields}\n        }};\n        self.persistence\n            .find_by_page(tenant_id, params.page, filter)\n            .await\n    }}\n}}\n",
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
        return "crate::next_id()?".into();
    }
    if field.name == "tenant_id" {
        return "tenant_id.to_owned()".into();
    }
    if let Some(audit) = &resource.audit
        && (audit.created_at == field.name || audit.updated_at == field.name)
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
        return rust_literal(default, field.value_type);
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
        .filter(|field| field.usage.update)
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
        .chain(resource.audit.iter().map(|audit| {
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
        .filter(|index| !update || business_index_fields(resource, index).iter().any(|field| field.usage.update))
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
