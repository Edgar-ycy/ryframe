use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use invoice_business::generated::{
    dto::memo_item::{CreateMemoItemDto, MemoItemListQuery, UpdateMemoItemDto},
    models::memo_item::{
        MemoItemFilter, MemoItemPersistencePort, MemoItemRecord, MemoItemTransaction,
    },
    services::memo_item::MemoItemService,
};
use ryframe_sdk::rust_decimal::Decimal;
use ryframe_sdk::serde_json::{self, json};
use ryframe_sdk::validator::Validate;
use ryframe_sdk::{
    ActorContext, AppError, AppResult, PageResult, PersistenceTransaction, TransactionAuditMode,
    ValidatedPageQuery,
};

fn create_input() -> CreateMemoItemDto {
    serde_json::from_value(json!({"code":"money","status":1,"tiny_count":1,"small_count":1,"byte_count":1,"short_count":1,"count":1})).unwrap()
}

#[test]
fn money_body_query_and_schema_stay_exact() {
    let body = json!({"price":"90071992547409.123456","discount":"9999999999999999999999.999999"});
    let input: UpdateMemoItemDto = serde_json::from_value(body).unwrap();
    input.validate().unwrap();
    assert_eq!(input.price.unwrap().to_string(), "90071992547409.123456");
    assert_eq!(
        input.discount.unwrap().to_string(),
        "9999999999999999999999.999999"
    );
    let query: MemoItemListQuery =
        serde_json::from_value(json!({"price":"90071992547409.123456"})).unwrap();
    let params = query
        .into_service_params(ryframe_sdk::PaginationPolicy::new(20, 100))
        .unwrap();
    assert_eq!(params.price.unwrap().to_string(), "90071992547409.123456");
    let schema =
        serde_json::to_value(<UpdateMemoItemDto as ryframe_sdk::utoipa::PartialSchema>::schema())
            .unwrap();
    assert_eq!(
        schema["properties"]["price"]["type"],
        json!(["string", "null"])
    );
    let missing = create_input();
    missing.validate().unwrap();
    assert!(missing.price.is_none());
    for value in [
        json!(0.125),
        json!("1e3"),
        json!("100000000000000.000000"),
        json!("1.1234567"),
    ] {
        match serde_json::from_value::<UpdateMemoItemDto>(json!({"price":value})) {
            Err(_) => {}
            Ok(input) => assert!(input.validate().is_err()),
        }
    }
    for value in ["100000000000000.000000", "1.1234567"] {
        let query: MemoItemListQuery = serde_json::from_value(json!({"price":value})).unwrap();
        assert!(
            query
                .into_service_params(ryframe_sdk::PaginationPolicy::new(20, 100))
                .is_err()
        );
    }
}

#[derive(Default)]
struct Counts {
    begins: AtomicUsize,
    reads: AtomicUsize,
    inserts: AtomicUsize,
    updates: AtomicUsize,
    commits: AtomicUsize,
    rollbacks: AtomicUsize,
}

#[derive(Clone, Default)]
struct Probe {
    counts: Arc<Counts>,
}
struct Transaction {
    probe: Probe,
}

#[ryframe_sdk::async_trait]
impl MemoItemPersistencePort for Probe {
    async fn find_by_id(&self, _tenant_id: &str, _id: u64) -> AppResult<Option<MemoItemRecord>> {
        self.counts.reads.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    async fn find_by_page(
        &self,
        _tenant_id: &str,
        _page: ValidatedPageQuery,
        _filter: MemoItemFilter,
    ) -> AppResult<PageResult<MemoItemRecord>> {
        self.counts.reads.fetch_add(1, Ordering::SeqCst);
        Err(AppError::Internal("未预期的查询".into()))
    }
    async fn begin(&self, _tenant_id: &str) -> AppResult<Box<dyn MemoItemTransaction>> {
        self.counts.begins.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Transaction {
            probe: self.clone(),
        }))
    }
}

#[ryframe_sdk::async_trait]
impl PersistenceTransaction for Transaction {
    async fn commit(self: Box<Self>, _mode: TransactionAuditMode) -> AppResult<()> {
        self.probe.counts.commits.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.probe.counts.rollbacks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[ryframe_sdk::async_trait]
impl MemoItemTransaction for Transaction {
    async fn find_by_id_for_update(
        &self,
        _tenant_id: &str,
        _id: u64,
    ) -> AppResult<Option<MemoItemRecord>> {
        self.probe.counts.reads.fetch_add(1, Ordering::SeqCst);
        Ok(Some(record()))
    }
    async fn insert(&self, mut record: MemoItemRecord) -> AppResult<MemoItemRecord> {
        self.probe.counts.inserts.fetch_add(1, Ordering::SeqCst);
        record.id = 42;
        Ok(record)
    }
    async fn update(&self, record: MemoItemRecord) -> AppResult<MemoItemRecord> {
        self.probe.counts.updates.fetch_add(1, Ordering::SeqCst);
        Ok(record)
    }
    async fn delete(&self, _tenant_id: &str, _id: u64) -> AppResult<()> {
        Err(AppError::Internal("未预期的删除".into()))
    }
}

fn record() -> MemoItemRecord {
    MemoItemRecord {
        id: 42,
        code: "money".into(),
        status: 1,
        tiny_count: 1,
        small_count: 1,
        byte_count: 1,
        short_count: 1,
        count: 1,
        large_count: u64::MAX,
        price: Decimal::from_str_exact("0.125000").unwrap(),
        discount: None,
        created_at: ryframe_sdk::chrono::Utc::now(),
        updated_at: ryframe_sdk::chrono::Utc::now(),
    }
}

fn actor() -> ActorContext {
    serde_json::from_value(json!({"user_id":1,"tenant_id":"system","username":"tester","dept_id":null,"dept_path":null,"data_scope":"all","custom_dept_ids":[],"include_self":false,"data_scope_version":1,"is_super_admin":true})).unwrap()
}

#[test]
fn direct_sdk_calls_reject_rounding_and_preserve_default_values() {
    ready(async {
        let probe = Arc::new(Probe::default());
        let service = MemoItemService::new(probe.clone());
        let actor = actor();
        let invalid = Decimal::from_str_exact("100000000000000.000000").unwrap();
        let mut input = create_input();
        input.price = Some(invalid);
        assert!(matches!(
            service.create(&actor, input.into()).await,
            Err(AppError::Validation(_))
        ));
        assert_eq!(probe.counts.begins.load(Ordering::SeqCst), 0);
        let input: UpdateMemoItemDto =
            serde_json::from_value(json!({"price":invalid.to_string()})).unwrap();
        assert!(matches!(
            service.update(&actor, 42, input.into()).await,
            Err(AppError::Validation(_))
        ));
        assert_eq!(probe.counts.updates.load(Ordering::SeqCst), 0);
        assert_eq!(probe.counts.rollbacks.load(Ordering::SeqCst), 1);
        let query: MemoItemListQuery = serde_json::from_value(json!({})).unwrap();
        let mut params = query
            .into_service_params(ryframe_sdk::PaginationPolicy::new(20, 100))
            .unwrap();
        params.price = Some(invalid);
        let reads = probe.counts.reads.load(Ordering::SeqCst);
        assert!(matches!(
            service.find_by_page(&actor, params).await,
            Err(AppError::Validation(_))
        ));
        assert_eq!(probe.counts.reads.load(Ordering::SeqCst), reads);
        let saved = service.create(&actor, create_input().into()).await.unwrap();
        assert_eq!(saved.price.to_string(), "0.125000");
        assert_eq!(saved.large_count, u64::MAX);
        assert_eq!(probe.counts.inserts.load(Ordering::SeqCst), 1);
        assert_eq!(probe.counts.commits.load(Ordering::SeqCst), 1);
    });
}

fn ready<F: std::future::Future>(future: F) -> F::Output {
    let waker = std::task::Waker::noop();
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(waker))
    {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("纯内存端口的用例不应等待外部资源"),
    }
}
