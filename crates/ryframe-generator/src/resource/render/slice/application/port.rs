use super::super::{ResourceIr, StorageKind};
use super::{business_index_fields, method_arguments, unique_business_indexes, unique_method_name};

pub(crate) fn port(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let control_methods = if resource.storage == StorageKind::ControlRow {
        let unique_methods = unique_business_indexes(resource)
            .map(|index| {
                let fields = business_index_fields(resource, index);
                format!(
                    "\n    async fn {method}(\n        &self,\n        tenant_id: &str,\n{arguments}        exclude_id: Option<i64>,\n    ) -> AppResult<Option<{pascal}Record>>;\n",
                    method = unique_method_name(index),
                    arguments = method_arguments(&fields),
                )
            })
            .collect::<String>();
        format!(
            "    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()>;\n{unique_methods}\n    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()>;\n\n"
        )
    } else {
        String::new()
    };
    format!(
        "{header}use async_trait::async_trait;\nuse ryframe_kernel::{{AppResult, PageResult}};\n\nuse crate::PersistenceTransaction;\n\nuse super::model::{{{pascal}Filter, {pascal}Record}};\n\n#[async_trait]\npub trait {pascal}Transaction: PersistenceTransaction + Sync {{\n{control_methods}    async fn find_by_id_for_update(\n        &self,\n        tenant_id: &str,\n        id: i64,\n    ) -> AppResult<Option<{pascal}Record>>;\n\n    async fn insert(&self, record: {pascal}Record) -> AppResult<{pascal}Record>;\n\n    async fn update(&self, record: {pascal}Record) -> AppResult<{pascal}Record>;\n\n    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()>;\n}}\n\n#[async_trait]\npub trait {pascal}PersistencePort: Send + Sync {{\n    async fn find_by_id(\n        &self,\n        tenant_id: &str,\n        id: i64,\n    ) -> AppResult<Option<{pascal}Record>>;\n\n    async fn find_by_page(\n        &self,\n        tenant_id: &str,\n        page: ryframe_kernel::ValidatedPageQuery,\n        filter: {pascal}Filter<'_>,\n    ) -> AppResult<PageResult<{pascal}Record>>;\n\n    async fn begin(&self, tenant_id: &str) -> AppResult<Box<dyn {pascal}Transaction>>;\n}}\n"
    )
}
