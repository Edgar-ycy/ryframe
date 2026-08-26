use super::super::{
    FieldIr, IndexIr, ResourceIr, StorageKind, ValueType, column_variant, rust_literal,
    uses_partial_text_filter,
};

pub(crate) fn unique_conflict_cases(resource: &ResourceIr) -> String {
    resource
        .indexes
        .iter()
        .filter(|index| index.unique && index.fields != resource.primary_key)
        .map(|index| {
            let labels = business_index_fields(resource, index)
                .iter()
                .map(|field| field.labels.zh_cn.as_str())
                .collect::<Vec<_>>()
                .join("、");
            let message = if labels.is_empty() {
                format!("{}已存在", resource.labels.zh_cn)
            } else {
                format!("{labels}已存在")
            };
            format!(
                "        if normalized.contains({index:?}) {{\n            return AppError::Conflict({message:?}.into());\n        }}",
                index = index.name.to_ascii_lowercase(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn control_transaction_methods(resource: &ResourceIr) -> String {
    if resource.storage != StorageKind::ControlRow {
        return String::new();
    }
    let pascal = &resource.pascal_name;
    let unique_methods = unique_business_indexes(resource)
        .map(|index| {
            let fields = business_index_fields(resource, index);
            let filters = fields
                .iter()
                .map(|field| {
                    format!(
                        "            .filter(entity::Column::{}.eq({}))",
                        column_variant(&field.name),
                        field.name,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let soft_delete = resource
                .soft_delete
                .as_ref()
                .map(|soft_delete| {
                    format!(
                        "\n            .filter(entity::Column::{}.eq({}))",
                        column_variant(&soft_delete.field),
                        field_literal(resource, &soft_delete.field, &soft_delete.active),
                    )
                })
                .unwrap_or_default();
            format!(
                "    async fn {method}(\n        &self,\n        tenant_id: &str,\n{arguments}        exclude_id: Option<i64>,\n    ) -> AppResult<Option<{pascal}Record>> {{\n        self.ensure_tenant(tenant_id)?;\n        let mut select = entity::Entity::find()\n            .filter(entity::Column::TenantId.eq(tenant_id))\n{filters}{soft_delete};\n        if let Some(exclude_id) = exclude_id {{\n            select = select.filter(entity::Column::Id.ne(exclude_id));\n        }}\n        Ok(select\n            .lock(LockType::Update)\n            .one(&self.transaction)\n            .await\n            .db()?\n            .map(to_record))\n    }}\n\n",
                method = unique_method_name(index),
                arguments = method_arguments(&fields),
            )
        })
        .collect::<String>();
    format!(
        "    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()> {{\n        self.ensure_tenant(tenant_id)?;\n        crate::TenantConfigTransferRepository\n            .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)\n            .await\n            .map(|_| ())\n    }}\n\n{unique_methods}    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()> {{\n        self.ensure_tenant(tenant_id)?;\n        crate::TenantConfigTransferRepository\n            .increment_configuration_version_in_txn(&self.transaction, tenant_id)\n            .await\n            .map(|_| ())\n    }}\n"
    )
}

pub(crate) struct PersistenceParts {
    pub(crate) declaration: String,
    pub(crate) read_connection: String,
    pub(crate) begin_transaction: String,
}

pub(crate) fn persistence_parts(resource: &ResourceIr) -> PersistenceParts {
    let pascal = &resource.pascal_name;
    match resource.storage {
        StorageKind::ControlRow => PersistenceParts {
            declaration: format!(
                "pub fn port(database: crate::ControlDatabaseCluster) -> Arc<dyn {pascal}PersistencePort> {{\n    Arc::new(Database{pascal}Persistence {{ database }})\n}}\n\nstruct Database{pascal}Persistence {{\n    database: crate::ControlDatabaseCluster,\n}}\n\nstruct Database{pascal}Transaction {{\n    tenant_id: String,\n    transaction: DatabaseTransaction,\n}}"
            ),
            read_connection: "        let database = self\n            .database\n            .select_read(crate::ReadConsistency::Eventual)\n            .connection;"
                .into(),
            begin_transaction: "        let transaction = self\n            .database\n            .write()\n            .begin()\n            .await\n            .db()?;"
                .into(),
        },
        StorageKind::TenantData => PersistenceParts {
            declaration: format!(
                "pub fn port(router: Arc<crate::TenantDatabaseRouter>) -> Arc<dyn {pascal}PersistencePort> {{\n    Arc::new(Database{pascal}Persistence {{ router }})\n}}\n\nstruct Database{pascal}Persistence {{\n    router: Arc<crate::TenantDatabaseRouter>,\n}}\n\nstruct Database{pascal}Transaction {{\n    tenant_id: String,\n    transaction: DatabaseTransaction,\n}}"
            ),
            read_connection: "        let session = self.router.resolve(tenant_id).await.map_err(tenant_data_error)?;\n        let database = session\n            .select_read(ryframe_db::ReadConsistency::Eventual)\n            .await\n            .map_err(tenant_data_error)?\n            .connection;"
                .into(),
            begin_transaction: "        let session = self.router.resolve(tenant_id).await.map_err(tenant_data_error)?;\n        let transaction = session.begin_write().await.map_err(tenant_data_error)?;"
                .into(),
        },
    }
}

pub(crate) fn base_select(resource: &ResourceIr) -> String {
    let mut output = "        let mut select = entity::Entity::find();".to_owned();
    if resource
        .fields
        .iter()
        .any(|field| field.name == "tenant_id")
    {
        output
            .push_str("\n        select = select.filter(entity::Column::TenantId.eq(tenant_id));");
    }
    if let Some(soft_delete) = &resource.soft_delete {
        output.push_str(&format!(
            "\n        select = select.filter(entity::Column::{}.eq({}));",
            column_variant(&soft_delete.field),
            field_literal(resource, &soft_delete.field, &soft_delete.active),
        ));
    }
    output
}

pub(crate) fn id_query(resource: &ResourceIr, id: &str) -> String {
    let mut query = if resource.storage == StorageKind::TenantData {
        format!("entity::Entity::find_by_id((tenant_id.to_owned(), {id}))")
    } else {
        let mut query = format!("entity::Entity::find_by_id({id})");
        if resource
            .fields
            .iter()
            .any(|field| field.name == "tenant_id")
        {
            query.push_str("\n            .filter(entity::Column::TenantId.eq(tenant_id))");
        }
        query
    };
    if let Some(soft_delete) = &resource.soft_delete {
        query.push_str(&format!(
            "\n            .filter(entity::Column::{}.eq({}))",
            column_variant(&soft_delete.field),
            field_literal(resource, &soft_delete.field, &soft_delete.active),
        ));
    }
    query
}

pub(crate) fn filter_statements(resource: &ResourceIr) -> String {
    resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && field.name != "tenant_id")
        .map(|field| {
            let column = column_variant(&field.name);
            if uses_partial_text_filter(field) {
                format!(
                    "        if let Some(value) = filter.{0}.filter(|value| !value.is_empty()) {{\n            select = select.filter(entity::Column::{column}.contains(value));\n        }}",
                    field.name
                )
            } else if field.value_type == ValueType::String {
                format!(
                    "        if let Some(value) = filter.{0}.filter(|value| !value.is_empty()) {{\n            select = select.filter(entity::Column::{column}.eq(value));\n        }}",
                    field.name
                )
            } else {
                format!(
                    "        if let Some(value) = filter.{0} {{\n            select = select.filter(entity::Column::{column}.eq(value));\n        }}",
                    field.name
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn order_statements(resource: &ResourceIr) -> String {
    let field = resource
        .fields
        .iter()
        .find(|field| field.usage.sort && field.name != "tenant_id")
        .map(|field| field.name.as_str())
        .unwrap_or("id");
    if field == "id" {
        "        select = select.order_by_asc(entity::Column::Id);".into()
    } else {
        format!(
            "        select = select\n            .order_by_asc(entity::Column::{})\n            .order_by_asc(entity::Column::Id);",
            column_variant(field)
        )
    }
}

pub(crate) fn mapping(resource: &ResourceIr, source: &str) -> String {
    resource
        .fields
        .iter()
        .map(|field| format!("        {}: {source}.{},", field.name, field.name))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn delete_body(resource: &ResourceIr) -> String {
    let query = id_query(resource, "id");
    if let Some(soft_delete) = &resource.soft_delete {
        let audit = resource.audit.as_ref().expect("审计契约已由 IR 校验");
        let updated_at = resource
            .fields
            .iter()
            .find(|field| field.name == audit.updated_at)
            .expect("审计字段已由 IR 校验");
        let updated_value = if updated_at.nullable {
            "Some(chrono::Utc::now())"
        } else {
            "chrono::Utc::now()"
        };
        format!(
            "        let model = {query}\n            .one(&self.transaction)\n            .await\n            .db()?\n            .ok_or_else(|| AppError::NotFound(\"资源不存在\".into()))?;\n        let mut active: entity::ActiveModel = model.into();\n        active.{field} = Set({deleted});\n        active.{updated_at_field} = Set({updated_value});\n        active.update(&self.transaction).await.db()?;\n        Ok(())",
            field = soft_delete.field,
            deleted = field_literal(resource, &soft_delete.field, &soft_delete.deleted),
            updated_at_field = audit.updated_at,
        )
    } else {
        format!(
            "        let model = {query}\n            .one(&self.transaction)\n            .await\n            .db()?\n            .ok_or_else(|| AppError::NotFound(\"资源不存在\".into()))?;\n        model.delete(&self.transaction).await.db()?;\n        Ok(())"
        )
    }
}

pub(crate) fn commit_body(storage: StorageKind) -> &'static str {
    match storage {
        StorageKind::ControlRow => {
            "        match audit_mode {\n            TransactionAuditMode::CurrentRequest => {\n                crate::application_ports::audit::commit_current_audit(self.transaction).await\n            }\n            TransactionAuditMode::Skip => self.transaction.commit().await.db(),\n        }"
        }
        StorageKind::TenantData => {
            "        let _ = audit_mode;\n        self.transaction.commit().await.db()"
        }
    }
}

pub(crate) fn field_literal(resource: &ResourceIr, field: &str, value: &toml::Value) -> String {
    let value_type = resource
        .fields
        .iter()
        .find(|candidate| candidate.name == field)
        .expect("数据库字段已由 IR 验证")
        .value_type;
    rust_literal(value, value_type)
}

pub(crate) fn tenant_error_mapper() -> String {
    r#"fn tenant_data_error(error: crate::TenantDataError) -> AppError {
    let message = error.to_string();
    match error {
        crate::TenantDataError::InvalidConfiguration(_) => AppError::Config(message),
        crate::TenantDataError::InvalidTenantId(_) => AppError::Validation(message),
        crate::TenantDataError::StalePlacementGeneration { .. } => {
            AppError::StalePlacementGeneration(message)
        }
        crate::TenantDataError::TenantDataMaintenance { .. }
        | crate::TenantDataError::FenceRejected { .. } => {
            AppError::TenantDataMaintenance(message, 5)
        }
        crate::TenantDataError::DedicatedTargetOccupied { .. } => {
            AppError::TenantOperationConflict(message)
        }
        crate::TenantDataError::UnknownTarget { .. }
        | crate::TenantDataError::TargetUnavailable { .. }
        | crate::TenantDataError::PlacementUnavailable { .. }
        | crate::TenantDataError::InvalidPlacement { .. }
        | crate::TenantDataError::PoolCapacityExhausted { .. }
        | crate::TenantDataError::ConnectionBudgetExhausted { .. } => {
            AppError::TenantDataTargetUnavailable(message, 5)
        }
    }
}
"#
    .into()
}

pub(crate) fn unique_business_indexes(resource: &ResourceIr) -> impl Iterator<Item = &IndexIr> {
    resource.indexes.iter().filter(|index| {
        index.unique
            && index.fields != resource.primary_key
            && !business_index_fields(resource, index).is_empty()
            && business_index_fields(resource, index)
                .iter()
                .all(|field| !field.nullable)
    })
}

pub(crate) fn business_index_fields<'a>(
    resource: &'a ResourceIr,
    index: &IndexIr,
) -> Vec<&'a FieldIr> {
    index
        .fields
        .iter()
        .filter(|name| name.as_str() != "tenant_id")
        .map(|name| {
            resource
                .fields
                .iter()
                .find(|field| field.name == *name)
                .expect("索引字段已由 IR 校验")
        })
        .collect()
}

pub(crate) fn unique_method_name(index: &IndexIr) -> String {
    format!(
        "find_by_{}_for_update",
        index
            .fields
            .iter()
            .filter(|field| field.as_str() != "tenant_id")
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("_and_")
    )
}

pub(crate) fn method_arguments(fields: &[&FieldIr]) -> String {
    fields
        .iter()
        .map(|field| {
            let argument_type = if field.value_type == ValueType::String {
                "&str"
            } else {
                field.rust_type.as_str()
            };
            format!("        {}: {argument_type},\n", field.name)
        })
        .collect()
}
