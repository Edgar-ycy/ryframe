{header}use std::collections::{{{collections}}};
use std::sync::{{Arc, Mutex, MutexGuard}};

use async_trait::async_trait;
use ryframe_kernel::{{AppError, AppResult, PageResult, ValidatedPageQuery}};

use crate::{{PersistenceTransaction, TransactionAuditMode}};

use super::model::{{{pascal}Filter, {pascal}Record{detail_import}}};
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
{owner_state}}}

#[derive(Clone, Default)]
pub struct {pascal}FakePersistence {{
    state: Arc<Mutex<FakeState>>,
}}

impl {pascal}FakePersistence {{
    pub fn insert_record(&self, tenant_id: impl Into<String>, record: {pascal}Record) {{
        self.lock().records.insert((tenant_id.into(), record.id), record);
    }}
{owner_setter}

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
{owner_helper}

#[async_trait]
impl {pascal}PersistencePort for {pascal}FakePersistence {{
    async fn find_by_id(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<{detail_type}>> {{
        let mut state = self.lock();
        state.calls.push({pascal}Call::FindById {{ tenant_id: tenant_id.into(), id }});
        fail_if_requested(&mut state, {pascal}Failure::FindById)?;
        Ok(state
            .records
            .get(&(tenant_id.into(), id)){active_record_filter_method}
            .cloned(){detail_mapping})
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
{owner_filter}
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
