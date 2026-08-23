use super::{ResourceIr, StorageKind, rust_base_type, rust_literal, uses_partial_text_filter};

pub(super) fn model(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let mut output = format!("{header}use ryframe_kernel::ValidatedPageQuery;\n\n");
    output.push_str("#[derive(Clone, Debug, PartialEq)]\n");
    output.push_str(&format!("pub struct {pascal}Record {{\n"));
    for field in &resource.fields {
        output.push_str(&format!("    pub {}: {},\n", field.name, field.rust_type));
    }
    output.push_str("}\n\n");

    output.push_str("#[derive(Clone, Debug)]\n");
    output.push_str(&format!("pub struct Create{pascal}Command {{\n"));
    for field in resource.fields.iter().filter(|field| field.usage.create) {
        output.push_str(&format!(
            "    pub {}: {},\n",
            field.name,
            command_type(field, field.usage.create_optional)
        ));
    }
    output.push_str("}\n\n");

    output.push_str("#[derive(Clone, Debug)]\n");
    output.push_str(&format!("pub struct Update{pascal}Command {{\n"));
    for field in resource.fields.iter().filter(|field| field.usage.update) {
        output.push_str(&format!(
            "    pub {}: {},\n",
            field.name,
            command_type(field, field.usage.update_optional)
        ));
    }
    output.push_str("}\n\n");

    output.push_str("#[derive(Clone, Debug)]\n");
    output.push_str(&format!("pub struct {pascal}ListParams {{\n"));
    output.push_str("    pub page: ValidatedPageQuery,\n");
    for field in resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && field.name != "tenant_id")
    {
        output.push_str(&format!(
            "    pub {}: Option<{}>,\n",
            field.name,
            rust_base_type(field.value_type)
        ));
    }
    output.push_str("}\n\n");

    output.push_str("#[derive(Clone, Copy, Debug, Default)]\n");
    output.push_str(&format!("pub struct {pascal}Filter<'a> {{\n"));
    for field in resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && field.name != "tenant_id")
    {
        let value_type = if matches!(field.value_type, super::ValueType::String) {
            "&'a str".to_owned()
        } else {
            rust_base_type(field.value_type).to_owned()
        };
        output.push_str(&format!("    pub {}: Option<{value_type}>,\n", field.name));
    }
    output.push_str("}\n");
    output
}

pub(super) fn port(resource: &ResourceIr, header: &str) -> String {
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

pub(super) fn service(resource: &ResourceIr, header: &str) -> String {
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
    let updates = resource
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
        .collect::<Vec<_>>()
        .join("\n");
    let filter_fields = resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && field.name != "tenant_id")
        .map(|field| {
            if matches!(field.value_type, super::ValueType::String) {
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
    let (create_before, create_after, write_before, update_unique_checks, write_after) = if resource
        .storage
        == StorageKind::ControlRow
    {
        let unique_checks = unique_business_indexes(resource)
                .map(|index| {
                    let fields = business_index_fields(resource, index);
                    let arguments = fields
                        .iter()
                        .map(|field| argument_expression(field, &format!("record.{}", field.name)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let label = fields
                        .iter()
                        .map(|field| field.labels.zh_cn.as_str())
                        .collect::<Vec<_>>()
                        .join("、");
                    format!(
                        "            if transaction.{method}(tenant_id, {arguments}, None).await?.is_some() {{\n                return Err(AppError::Conflict({message:?}.into()));\n            }}",
                        method = unique_method_name(index),
                        message = format!("{label}已存在"),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
        let update_unique_checks = unique_business_indexes(resource)
            .filter(|index| {
                business_index_fields(resource, index)
                    .iter()
                    .any(|field| field.usage.update)
            })
            .map(|index| {
                let fields = business_index_fields(resource, index);
                let arguments = fields
                    .iter()
                    .map(|field| argument_expression(field, &format!("record.{}", field.name)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let label = fields
                    .iter()
                    .map(|field| field.labels.zh_cn.as_str())
                    .collect::<Vec<_>>()
                    .join("、");
                format!(
                    "            if transaction.{method}(tenant_id, {arguments}, Some(id)).await?.is_some() {{\n                return Err(AppError::Conflict({message:?}.into()));\n            }}",
                    method = unique_method_name(index),
                    message = format!("{label}已存在"),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        (
            format!(
                "            transaction.lock_configuration(tenant_id).await?;\n{unique_checks}"
            ),
            "            transaction.increment_configuration_version(tenant_id).await?;".to_owned(),
            "            transaction.lock_configuration(tenant_id).await?;".to_owned(),
            update_unique_checks,
            "            transaction.increment_configuration_version(tenant_id).await?;".to_owned(),
        )
    } else {
        (
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        )
    };
    let updates = format!("{updates}\n{update_unique_checks}");
    format!(
        "{header}use std::sync::Arc;\n\nuse chrono::Utc;\nuse ryframe_kernel::{{ActorContext, AppError, AppResult, PageResult}};\n\nuse crate::{{TransactionAuditMode, complete_transaction}};\n\nuse super::model::{{Create{pascal}Command, {pascal}Filter, {pascal}ListParams, {pascal}Record, Update{pascal}Command}};\nuse super::port::{pascal}PersistencePort;\n\npub struct {pascal}Service {{\n    persistence: Arc<dyn {pascal}PersistencePort>,\n}}\n\nimpl {pascal}Service {{\n    pub fn new(persistence: Arc<dyn {pascal}PersistencePort>) -> Self {{\n        Self {{ persistence }}\n    }}\n\n    pub async fn find_by_id(\n        &self,\n        actor: &ActorContext,\n        id: i64,\n    ) -> AppResult<Option<{pascal}Record>> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n        self.persistence.find_by_id(tenant_id, id).await\n    }}\n\n    pub async fn create(\n        &self,\n        actor: &ActorContext,\n        command: Create{pascal}Command,\n    ) -> AppResult<{pascal}Record> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n        let now = Utc::now();\n        let record = {pascal}Record {{\n{record_fields}\n        }};\n        let transaction = self.persistence.begin(tenant_id).await?;\n        let operation = async {{\n{create_before}\n            let saved = transaction.insert(record).await?;\n{create_after}\n            Ok(saved)\n        }}\n        .await;\n        complete_transaction(transaction, operation, {audit_mode}).await\n    }}\n\n    pub async fn update(\n        &self,\n        actor: &ActorContext,\n        id: i64,\n        command: Update{pascal}Command,\n    ) -> AppResult<{pascal}Record> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n        let transaction = self.persistence.begin(tenant_id).await?;\n        let operation = async {{\n{write_before}\n            let mut record = transaction\n                .find_by_id_for_update(tenant_id, id)\n                .await?\n                .ok_or_else(|| AppError::NotFound({not_found:?}.into()))?;\n            let now = Utc::now();\n{updates}\n            let saved = transaction.update(record).await?;\n{write_after}\n            Ok(saved)\n        }}\n        .await;\n        complete_transaction(transaction, operation, {audit_mode}).await\n    }}\n\n    pub async fn delete(&self, actor: &ActorContext, id: i64) -> AppResult<()> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n        let transaction = self.persistence.begin(tenant_id).await?;\n        let operation = async {{\n{write_before}\n            transaction\n                .find_by_id_for_update(tenant_id, id)\n                .await?\n                .ok_or_else(|| AppError::NotFound({not_found:?}.into()))?;\n            transaction.delete(tenant_id, id).await?;\n{write_after}\n            Ok(())\n        }}\n        .await;\n        complete_transaction(transaction, operation, {audit_mode}).await\n    }}\n\n    pub async fn find_by_page(\n        &self,\n        actor: &ActorContext,\n        params: {pascal}ListParams,\n    ) -> AppResult<PageResult<{pascal}Record>> {{\n        let tenant_id = crate::validated_tenant_id(actor)?;\n        let filter = {pascal}Filter {{\n{filter_fields}\n        }};\n        self.persistence\n            .find_by_page(tenant_id, params.page, filter)\n            .await\n    }}\n}}\n",
    )
}

pub(super) fn fake(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let tenant_mismatch = format!("{}事务租户不匹配", resource.labels.zh_cn);
    let (active_record_filter_method, active_record_filter) = fake_active_record_filters(resource);
    let record_order = fake_record_order(resource);
    let filters = resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && field.name != "tenant_id")
        .map(|field| {
            let value = if field.value_type == super::ValueType::String {
                format!(
                    "filter.{0}.filter(|value| !value.is_empty())",
                    field.name
                )
            } else {
                format!("filter.{}", field.name)
            };
            let mismatch = if uses_partial_text_filter(field) {
                if field.nullable {
                    format!(
                        "!record.{0}.as_deref().is_some_and(|candidate| candidate.contains(value))",
                        field.name
                    )
                } else {
                    format!("!record.{}.contains(value)", field.name)
                }
            } else if field.nullable {
                if field.value_type == super::ValueType::String {
                    format!("record.{}.as_deref() != Some(value)", field.name)
                } else {
                    format!("record.{} != Some(value)", field.name)
                }
            } else {
                format!("record.{} != value", field.name)
            };
            format!(
                "            if let Some(value) = {value}\n                && {mismatch}\n            {{\n                return None;\n            }}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let (control_call_variants, control_failure_variants, control_methods) =
        fake_control_parts(resource);
    format!(
        r#"{header}use std::collections::{{BTreeMap, VecDeque}};
use std::sync::{{Arc, Mutex, MutexGuard}};

use async_trait::async_trait;
use ryframe_kernel::{{AppError, AppResult, PageResult, ValidatedPageQuery}};

use crate::{{PersistenceTransaction, TransactionAuditMode}};

use super::model::{{{pascal}Filter, {pascal}Record}};
use super::port::{{{pascal}PersistencePort, {pascal}Transaction}};

#[derive(Clone, Debug, PartialEq)]
pub enum {pascal}Call {{
    FindById {{ tenant_id: String, id: i64 }},
    FindByPage {{ tenant_id: String, page: u64, page_size: u64 }},
    Begin {{ tenant_id: String }},
{control_call_variants}
    FindByIdForUpdate {{ tenant_id: String, id: i64 }},
    Insert {{ record: {pascal}Record }},
    Update {{ record: {pascal}Record }},
    Delete {{ tenant_id: String, id: i64 }},
    Commit {{ audit_mode: TransactionAuditMode }},
    Rollback,
}}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum {pascal}TransactionState {{
    Active,
    Committed,
    RolledBack,
}}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum {pascal}Failure {{
    FindById,
    FindByPage,
    Begin,
{control_failure_variants}
    FindByIdForUpdate,
    Insert,
    Update,
    Delete,
    Commit,
    Rollback,
}}

#[derive(Default)]
struct FakeState {{
    calls: Vec<{pascal}Call>,
    records: BTreeMap<(String, i64), {pascal}Record>,
    failures: VecDeque<{pascal}Failure>,
    transactions: Vec<{pascal}TransactionState>,
}}

#[derive(Clone, Default)]
pub struct {pascal}FakePersistence {{
    state: Arc<Mutex<FakeState>>,
}}

impl {pascal}FakePersistence {{
    pub fn insert_record(&self, tenant_id: impl Into<String>, record: {pascal}Record) {{
        self.lock().records.insert((tenant_id.into(), record.id), record);
    }}

    pub fn fail_next(&self, failure: {pascal}Failure) {{
        self.lock().failures.push_back(failure);
    }}

    pub fn calls(&self) -> Vec<{pascal}Call> {{
        self.lock().calls.clone()
    }}

    pub fn transaction_states(&self) -> Vec<{pascal}TransactionState> {{
        self.lock().transactions.clone()
    }}

    fn lock(&self) -> MutexGuard<'_, FakeState> {{
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }}
}}

fn fail_if_requested(
    state: &mut FakeState,
    failure: {pascal}Failure,
) -> AppResult<()> {{
    if state.failures.front() == Some(&failure) {{
        state.failures.pop_front();
        return Err(AppError::Database(format!("fake failure: {{failure:?}}")));
    }}
    Ok(())
}}

#[async_trait]
impl {pascal}PersistencePort for {pascal}FakePersistence {{
    async fn find_by_id(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<{pascal}Record>> {{
        let mut state = self.lock();
        state.calls.push({pascal}Call::FindById {{ tenant_id: tenant_id.into(), id }});
        fail_if_requested(&mut state, {pascal}Failure::FindById)?;
        Ok(state
            .records
            .get(&(tenant_id.into(), id)){active_record_filter_method}
            .cloned())
    }}

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: {pascal}Filter<'_>,
    ) -> AppResult<PageResult<{pascal}Record>> {{
        let mut state = self.lock();
        state.calls.push({pascal}Call::FindByPage {{
            tenant_id: tenant_id.into(),
            page: page.page(),
            page_size: page.page_size(),
        }});
        fail_if_requested(&mut state, {pascal}Failure::FindByPage)?;
        let mut records = state.records.iter().filter_map(|((owner, _), record)| {{
            if owner != tenant_id {{ return None; }}
{active_record_filter}
{filters}
            Some(record.clone())
        }}).collect::<Vec<_>>();
{record_order}
        let total = records.len() as u64;
        let start = usize::try_from(page.offset()).unwrap_or(usize::MAX).min(records.len());
        let end = start.saturating_add(page.page_size() as usize).min(records.len());
        Ok(PageResult::new(records[start..end].to_vec(), total, &page))
    }}

    async fn begin(&self, tenant_id: &str) -> AppResult<Box<dyn {pascal}Transaction>> {{
        let mut state = self.lock();
        state.calls.push({pascal}Call::Begin {{ tenant_id: tenant_id.into() }});
        fail_if_requested(&mut state, {pascal}Failure::Begin)?;
        let transaction_index = state.transactions.len();
        state.transactions.push({pascal}TransactionState::Active);
        let original: BTreeMap<i64, {pascal}Record> = state.records.iter()
            .filter(|((owner, _), _)| owner == tenant_id)
            .map(|((_, id), record)| (*id, record.clone()))
            .collect();
        Ok(Box::new(FakeTransaction {{
            state: Arc::clone(&self.state),
            tenant_id: tenant_id.into(),
            transaction_index,
            view: Mutex::new(original.clone()),
            original,
        }}))
    }}
}}

struct FakeTransaction {{
    state: Arc<Mutex<FakeState>>,
    tenant_id: String,
    transaction_index: usize,
    view: Mutex<BTreeMap<i64, {pascal}Record>>,
    original: BTreeMap<i64, {pascal}Record>,
}}

impl FakeTransaction {{
    fn ensure_tenant(&self, tenant_id: &str) -> AppResult<()> {{
        if self.tenant_id == tenant_id {{
            Ok(())
        }} else {{
            Err(AppError::Authorization({tenant_mismatch:?}.into()))
        }}
    }}

    fn lock_state(&self) -> MutexGuard<'_, FakeState> {{
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }}

    fn lock_view(&self) -> MutexGuard<'_, BTreeMap<i64, {pascal}Record>> {{
        self.view.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }}
}}

#[async_trait]
impl {pascal}Transaction for FakeTransaction {{
{control_methods}
    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<{pascal}Record>> {{
        self.ensure_tenant(tenant_id)?;
        {{
            let mut state = self.lock_state();
            state.calls.push({pascal}Call::FindByIdForUpdate {{ tenant_id: tenant_id.into(), id }});
            fail_if_requested(&mut state, {pascal}Failure::FindByIdForUpdate)?;
        }}
        Ok(self.lock_view().get(&id).cloned())
    }}

    async fn insert(&self, record: {pascal}Record) -> AppResult<{pascal}Record> {{
        self.ensure_tenant(&record.tenant_id)?;
        {{
            let mut state = self.lock_state();
            state.calls.push({pascal}Call::Insert {{ record: record.clone() }});
            fail_if_requested(&mut state, {pascal}Failure::Insert)?;
        }}
        self.lock_view().insert(record.id, record.clone());
        Ok(record)
    }}

    async fn update(&self, record: {pascal}Record) -> AppResult<{pascal}Record> {{
        self.ensure_tenant(&record.tenant_id)?;
        {{
            let mut state = self.lock_state();
            state.calls.push({pascal}Call::Update {{ record: record.clone() }});
            fail_if_requested(&mut state, {pascal}Failure::Update)?;
        }}
        self.lock_view().insert(record.id, record.clone());
        Ok(record)
    }}

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()> {{
        self.ensure_tenant(tenant_id)?;
        {{
            let mut state = self.lock_state();
            state.calls.push({pascal}Call::Delete {{ tenant_id: tenant_id.into(), id }});
            fail_if_requested(&mut state, {pascal}Failure::Delete)?;
        }}
        self.lock_view().remove(&id);
        Ok(())
    }}
}}

#[async_trait]
impl PersistenceTransaction for FakeTransaction {{
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {{
        let view = self.lock_view().clone();
        let mut state = self.lock_state();
        state.calls.push({pascal}Call::Commit {{ audit_mode }});
        fail_if_requested(&mut state, {pascal}Failure::Commit)?;
        for id in self.original.keys().filter(|id| !view.contains_key(id)) {{
            state.records.remove(&(self.tenant_id.clone(), *id));
        }}
        for (id, record) in view {{
            if self.original.get(&id) != Some(&record) {{
                state.records.insert((self.tenant_id.clone(), id), record);
            }}
        }}
        state.transactions[self.transaction_index] = {pascal}TransactionState::Committed;
        Ok(())
    }}

    async fn rollback(self: Box<Self>) -> AppResult<()> {{
        let mut state = self.lock_state();
        state.calls.push({pascal}Call::Rollback);
        fail_if_requested(&mut state, {pascal}Failure::Rollback)?;
        state.transactions[self.transaction_index] = {pascal}TransactionState::RolledBack;
        Ok(())
    }}
}}
"#
    )
}

fn fake_active_record_filters(resource: &ResourceIr) -> (String, String) {
    let Some(soft_delete) = &resource.soft_delete else {
        return (String::new(), String::new());
    };
    let field = resource
        .fields
        .iter()
        .find(|field| field.name == soft_delete.field)
        .expect("软删除字段已经在 Resource IR 中校验");
    let active = match (&soft_delete.active, field.value_type) {
        (toml::Value::String(value), super::ValueType::String) => format!("{value:?}"),
        _ => rust_literal(&soft_delete.active, field.value_type),
    };
    (
        format!(
            "\n            .filter(|record| record.{} == {active})",
            field.name
        ),
        format!(
            "            if record.{} != {active} {{ return None; }}",
            field.name
        ),
    )
}

fn fake_record_order(resource: &ResourceIr) -> String {
    let field = resource
        .fields
        .iter()
        .find(|field| field.usage.sort && field.name != "tenant_id")
        .map(|field| field.name.as_str())
        .unwrap_or("id");
    if field == "id" {
        "        records.sort_by_key(|record| record.id);".to_owned()
    } else {
        format!(
            "        records.sort_by(|left, right| {{\n            left.{field}\n                .cmp(&right.{field})\n                .then_with(|| left.id.cmp(&right.id))\n        }});"
        )
    }
}

fn command_type(field: &super::FieldIr, optional: bool) -> String {
    let value_type = rust_base_type(field.value_type);
    if optional || field.nullable {
        format!("Option<{value_type}>")
    } else {
        value_type.to_owned()
    }
}

fn fake_control_parts(resource: &ResourceIr) -> (String, String, String) {
    if resource.storage != StorageKind::ControlRow {
        return (String::new(), String::new(), String::new());
    }
    let pascal = &resource.pascal_name;
    let mut calls = "    LockConfiguration { tenant_id: String },".to_owned();
    let mut failures = "    LockConfiguration,".to_owned();
    let mut methods = String::from(
        "    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()> {\n        self.ensure_tenant(tenant_id)?;\n        let mut state = self.lock_state();\n        state.calls.push(",
    );
    methods.push_str(&format!(
        "{pascal}Call::LockConfiguration {{ tenant_id: tenant_id.into() }});\n        fail_if_requested(&mut state, {pascal}Failure::LockConfiguration)\n    }}\n\n"
    ));
    for index in unique_business_indexes(resource) {
        let fields = business_index_fields(resource, index);
        let method = unique_method_name(index);
        let variant = crate::naming::to_pascal_case(&method);
        let variant = variant.trim_end_matches("ForUpdate");
        let call_fields = fields
            .iter()
            .map(|field| {
                let ty = if field.value_type == super::ValueType::String {
                    "String"
                } else {
                    field.rust_type.as_str()
                };
                format!("{}: {ty}", field.name)
            })
            .collect::<Vec<_>>()
            .join(", ");
        calls.push_str(&format!(
            "\n    {variant} {{ tenant_id: String, {call_fields}, exclude_id: Option<i64> }},"
        ));
        failures.push_str(&format!("\n    {variant},"));
        let arguments = method_arguments(&fields);
        let call_values = fields
            .iter()
            .map(|field| {
                if field.value_type == super::ValueType::String {
                    format!("{}: {}.into()", field.name, field.name)
                } else {
                    format!("{}: {}", field.name, field.name)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let comparisons = fields
            .iter()
            .map(|field| format!("record.{0} == {0}", field.name))
            .collect::<Vec<_>>()
            .join(" && ");
        methods.push_str(&format!(
            "    async fn {method}(\n        &self,\n        tenant_id: &str,\n{arguments}        exclude_id: Option<i64>,\n    ) -> AppResult<Option<{pascal}Record>> {{\n        self.ensure_tenant(tenant_id)?;\n        {{\n            let mut state = self.lock_state();\n            state.calls.push({pascal}Call::{variant} {{ tenant_id: tenant_id.into(), {call_values}, exclude_id }});\n            fail_if_requested(&mut state, {pascal}Failure::{variant})?;\n        }}\n        Ok(self.lock_view().values().find(|record| exclude_id != Some(record.id) && {comparisons}).cloned())\n    }}\n\n"
        ));
    }
    calls.push_str("\n    IncrementConfigurationVersion { tenant_id: String },");
    failures.push_str("\n    IncrementConfigurationVersion,");
    methods.push_str(&format!(
        "    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()> {{\n        self.ensure_tenant(tenant_id)?;\n        let mut state = self.lock_state();\n        state.calls.push({pascal}Call::IncrementConfigurationVersion {{ tenant_id: tenant_id.into() }});\n        fail_if_requested(&mut state, {pascal}Failure::IncrementConfigurationVersion)\n    }}\n"
    ));
    (calls, failures, methods)
}

fn create_expression(resource: &ResourceIr, field: &super::FieldIr) -> String {
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
            return if field.value_type == super::ValueType::String {
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
            let argument_type = if field.value_type == super::ValueType::String {
                "&str".to_owned()
            } else {
                rust_base_type(field.value_type).to_owned()
            };
            format!("        {}: {argument_type},\n", field.name)
        })
        .collect()
}

fn argument_expression(field: &super::FieldIr, expression: &str) -> String {
    if field.value_type == super::ValueType::String {
        format!("&{expression}")
    } else {
        expression.to_owned()
    }
}
