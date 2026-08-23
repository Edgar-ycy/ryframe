use super::{
    ResourceIr, StorageKind, ValueType, column_variant, rust_literal, uses_partial_text_filter,
};

pub(super) fn entity(resource: &ResourceIr, header: &str) -> String {
    let mut output = format!(
        "{header}use sea_orm::entity::prelude::*;\n\n#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]\n#[sea_orm(table_name = {:?})]\npub struct Model {{\n",
        resource.table
    );
    for field in &resource.fields {
        if resource.primary_key.contains(&field.name) {
            output.push_str("    #[sea_orm(primary_key, auto_increment = false)]\n");
        }
        output.push_str(&format!("    pub {}: {},\n", field.name, field.rust_type));
    }
    output.push_str("}\n\n");
    output.push_str(&soft_delete_constants(resource));
    output.push_str(
        "#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]\npub enum Relation {}\n\nimpl ActiveModelBehavior for ActiveModel {}\n",
    );
    output
}

pub(super) fn repository(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let name = &resource.name;
    let persistence = persistence_parts(resource);
    let base_select = base_select(resource);
    let id_query = id_query(resource, "id");
    let filters = filter_statements(resource);
    let order = order_statements(resource);
    let to_record = mapping(resource, "model");
    let to_entity = mapping(resource, "record");
    let delete = delete_body(resource);
    let commit = commit_body(resource.storage);
    let unique_conflicts = unique_conflict_cases(resource);
    let generic_conflict = format!("{}已存在", resource.labels.zh_cn);
    let tenant_mismatch = format!("{}事务租户不匹配", resource.labels.zh_cn);
    let tenant_error = if resource.storage == StorageKind::TenantData {
        tenant_error_mapper()
    } else {
        String::new()
    };
    let pagination = match resource.storage {
        StorageKind::ControlRow => "crate::pagination::paginate",
        StorageKind::TenantData => "ryframe_db::pagination::paginate",
    };
    let model_trait = if resource.soft_delete.is_none() {
        "ModelTrait, "
    } else {
        ""
    };
    let transaction_trait = if resource.storage == StorageKind::ControlRow {
        "TransactionTrait, "
    } else {
        ""
    };
    let control_transaction_methods = control_transaction_methods(resource);
    format!(
        r#"{header}use std::sync::Arc;

use async_trait::async_trait;
use ryframe_application::generated::{name}::{{
    {pascal}Filter, {pascal}PersistencePort, {pascal}Record, {pascal}Transaction,
}};
use ryframe_application::{{PersistenceTransaction, TransactionAuditMode}};
use ryframe_kernel::{{AppError, AppResult, PageResult, ValidatedPageQuery}};
use sea_orm::{{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseTransaction, EntityTrait,
    {model_trait}QueryFilter, QueryOrder, QuerySelect, {transaction_trait}sea_query::LockType,
}};

use super::entity;

{declaration}

impl Database{pascal}Transaction {{
    fn ensure_tenant(&self, tenant_id: &str) -> AppResult<()> {{
        if self.tenant_id == tenant_id {{
            Ok(())
        }} else {{
            Err(AppError::Authorization({tenant_mismatch:?}.into()))
        }}
    }}
}}

#[async_trait]
impl {pascal}PersistencePort for Database{pascal}Persistence {{
    async fn find_by_id(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<{pascal}Record>> {{
{read_connection}
        Ok({id_query}
            .one(&database)
            .await
            .map_err(database_error)?
            .map(to_record))
    }}

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: {pascal}Filter<'_>,
    ) -> AppResult<PageResult<{pascal}Record>> {{
{read_connection}
{base_select}
{filters}
{order}
        let result = {pagination}(&database, select, &page).await?;
        Ok(PageResult::new(
            result.records.into_iter().map(to_record).collect(),
            result.total,
            &page,
        ))
    }}

    async fn begin(&self, tenant_id: &str) -> AppResult<Box<dyn {pascal}Transaction>> {{
{begin_transaction}
        Ok(Box::new(Database{pascal}Transaction {{
            tenant_id: tenant_id.to_owned(),
            transaction,
        }}))
    }}
}}

#[async_trait]
impl {pascal}Transaction for Database{pascal}Transaction {{
{control_transaction_methods}
    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<{pascal}Record>> {{
        self.ensure_tenant(tenant_id)?;
        Ok({id_query}
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map_err(database_error)?
            .map(to_record))
    }}

    async fn insert(&self, record: {pascal}Record) -> AppResult<{pascal}Record> {{
        self.ensure_tenant(&record.tenant_id)?;
        entity::ActiveModel::from(to_entity(record))
            .insert(&self.transaction)
            .await
            .map(to_record)
            .map_err(database_error)
    }}

    async fn update(&self, record: {pascal}Record) -> AppResult<{pascal}Record> {{
        self.ensure_tenant(&record.tenant_id)?;
        entity::ActiveModel::from(to_entity(record))
            .update(&self.transaction)
            .await
            .map(to_record)
            .map_err(database_error)
    }}

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()> {{
        self.ensure_tenant(tenant_id)?;
{delete}
    }}
}}

#[async_trait]
impl PersistenceTransaction for Database{pascal}Transaction {{
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {{
{commit}
    }}

    async fn rollback(self: Box<Self>) -> AppResult<()> {{
        self.transaction.rollback().await.map_err(database_error)
    }}
}}

fn to_record(model: entity::Model) -> {pascal}Record {{
    {pascal}Record {{
{to_record}
    }}
}}

fn to_entity(record: {pascal}Record) -> entity::Model {{
    entity::Model {{
{to_entity}
    }}
}}

fn database_error(error: sea_orm::DbErr) -> AppError {{
    let message = error.to_string();
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("1062") || normalized.contains("duplicate entry") {{
{unique_conflicts}
        return AppError::Conflict({generic_conflict:?}.into());
    }}
    AppError::Database(message)
}}

{tenant_error}"#,
        declaration = persistence.declaration,
        read_connection = persistence.read_connection,
        begin_transaction = persistence.begin_transaction,
        pagination = pagination,
        generic_conflict = generic_conflict,
    )
}

fn unique_conflict_cases(resource: &ResourceIr) -> String {
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

fn control_transaction_methods(resource: &ResourceIr) -> String {
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
                "    async fn {method}(\n        &self,\n        tenant_id: &str,\n{arguments}        exclude_id: Option<i64>,\n    ) -> AppResult<Option<{pascal}Record>> {{\n        self.ensure_tenant(tenant_id)?;\n        let mut select = entity::Entity::find()\n            .filter(entity::Column::TenantId.eq(tenant_id))\n{filters}{soft_delete};\n        if let Some(exclude_id) = exclude_id {{\n            select = select.filter(entity::Column::Id.ne(exclude_id));\n        }}\n        Ok(select\n            .lock(LockType::Update)\n            .one(&self.transaction)\n            .await\n            .map_err(database_error)?\n            .map(to_record))\n    }}\n\n",
                method = unique_method_name(index),
                arguments = method_arguments(&fields),
            )
        })
        .collect::<String>();
    format!(
        "    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()> {{\n        self.ensure_tenant(tenant_id)?;\n        crate::TenantConfigTransferRepository\n            .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)\n            .await\n            .map(|_| ())\n    }}\n\n{unique_methods}    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()> {{\n        self.ensure_tenant(tenant_id)?;\n        crate::TenantConfigTransferRepository\n            .increment_configuration_version_in_txn(&self.transaction, tenant_id)\n            .await\n            .map(|_| ())\n    }}\n"
    )
}

struct PersistenceParts {
    declaration: String,
    read_connection: String,
    begin_transaction: String,
}

fn persistence_parts(resource: &ResourceIr) -> PersistenceParts {
    let pascal = &resource.pascal_name;
    match resource.storage {
        StorageKind::ControlRow => PersistenceParts {
            declaration: format!(
                "pub fn port(database: crate::ControlDatabaseCluster) -> Arc<dyn {pascal}PersistencePort> {{\n    Arc::new(Database{pascal}Persistence {{ database }})\n}}\n\nstruct Database{pascal}Persistence {{\n    database: crate::ControlDatabaseCluster,\n}}\n\nstruct Database{pascal}Transaction {{\n    tenant_id: String,\n    transaction: DatabaseTransaction,\n}}"
            ),
            read_connection: "        let database = self\n            .database\n            .select_read(crate::ReadConsistency::Eventual)\n            .connection;"
                .into(),
            begin_transaction: "        let transaction = self\n            .database\n            .write()\n            .begin()\n            .await\n            .map_err(database_error)?;"
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

fn base_select(resource: &ResourceIr) -> String {
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

fn id_query(resource: &ResourceIr, id: &str) -> String {
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

fn filter_statements(resource: &ResourceIr) -> String {
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

fn soft_delete_constants(resource: &ResourceIr) -> String {
    let Some(soft_delete) = &resource.soft_delete else {
        return String::new();
    };
    let field = resource
        .fields
        .iter()
        .find(|field| field.name == soft_delete.field)
        .expect("软删字段引用已经在 IR 中校验");
    let (constant_type, active, deleted) = match field.value_type {
        ValueType::String => (
            "&str",
            soft_delete
                .active
                .as_str()
                .expect("string 软删值已校验")
                .to_owned(),
            soft_delete
                .deleted
                .as_str()
                .expect("string 软删值已校验")
                .to_owned(),
        ),
        ValueType::I32 => (
            "i32",
            soft_delete
                .active
                .as_integer()
                .expect("i32 软删值已校验")
                .to_string(),
            soft_delete
                .deleted
                .as_integer()
                .expect("i32 软删值已校验")
                .to_string(),
        ),
        ValueType::I64 => (
            "i64",
            soft_delete
                .active
                .as_integer()
                .expect("i64 软删值已校验")
                .to_string(),
            soft_delete
                .deleted
                .as_integer()
                .expect("i64 软删值已校验")
                .to_string(),
        ),
        ValueType::Bool => (
            "bool",
            soft_delete
                .active
                .as_bool()
                .expect("bool 软删值已校验")
                .to_string(),
            soft_delete
                .deleted
                .as_bool()
                .expect("bool 软删值已校验")
                .to_string(),
        ),
        _ => unreachable!("软删字段类型已经在 IR 中严格校验"),
    };
    let active = if field.value_type == ValueType::String {
        format!("{active:?}")
    } else {
        active
    };
    let deleted = if field.value_type == ValueType::String {
        format!("{deleted:?}")
    } else {
        deleted
    };
    format!(
        "pub const SOFT_DELETE_ACTIVE: {constant_type} = {active};\npub const SOFT_DELETE_DELETED: {constant_type} = {deleted};\n\n"
    )
}

fn order_statements(resource: &ResourceIr) -> String {
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

fn mapping(resource: &ResourceIr, source: &str) -> String {
    resource
        .fields
        .iter()
        .map(|field| format!("        {}: {source}.{},", field.name, field.name))
        .collect::<Vec<_>>()
        .join("\n")
}

fn delete_body(resource: &ResourceIr) -> String {
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
            "        let model = {query}\n            .one(&self.transaction)\n            .await\n            .map_err(database_error)?\n            .ok_or_else(|| AppError::NotFound(\"资源不存在\".into()))?;\n        let mut active: entity::ActiveModel = model.into();\n        active.{field} = Set({deleted});\n        active.{updated_at_field} = Set({updated_value});\n        active.update(&self.transaction).await.map_err(database_error)?;\n        Ok(())",
            field = soft_delete.field,
            deleted = field_literal(resource, &soft_delete.field, &soft_delete.deleted),
            updated_at_field = audit.updated_at,
        )
    } else {
        format!(
            "        let model = {query}\n            .one(&self.transaction)\n            .await\n            .map_err(database_error)?\n            .ok_or_else(|| AppError::NotFound(\"资源不存在\".into()))?;\n        model.delete(&self.transaction).await.map_err(database_error)?;\n        Ok(())"
        )
    }
}

fn commit_body(storage: StorageKind) -> &'static str {
    match storage {
        StorageKind::ControlRow => {
            "        match audit_mode {\n            TransactionAuditMode::CurrentRequest => {\n                crate::application_ports::audit::commit_current_audit(self.transaction).await\n            }\n            TransactionAuditMode::Skip => self.transaction.commit().await.map_err(database_error),\n        }"
        }
        StorageKind::TenantData => {
            "        let _ = audit_mode;\n        self.transaction.commit().await.map_err(database_error)"
        }
    }
}

fn field_literal(resource: &ResourceIr, field: &str, value: &toml::Value) -> String {
    let value_type = resource
        .fields
        .iter()
        .find(|candidate| candidate.name == field)
        .expect("数据库字段已由 IR 验证")
        .value_type;
    rust_literal(value, value_type)
}

fn tenant_error_mapper() -> String {
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

fn unique_business_indexes(resource: &ResourceIr) -> impl Iterator<Item = &super::IndexIr> {
    resource.indexes.iter().filter(|index| {
        index.unique
            && index.fields != resource.primary_key
            && !business_index_fields(resource, index).is_empty()
            && business_index_fields(resource, index)
                .iter()
                .all(|field| !field.nullable)
    })
}

fn business_index_fields<'a>(
    resource: &'a ResourceIr,
    index: &super::IndexIr,
) -> Vec<&'a super::FieldIr> {
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

fn unique_method_name(index: &super::IndexIr) -> String {
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

fn method_arguments(fields: &[&super::FieldIr]) -> String {
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
