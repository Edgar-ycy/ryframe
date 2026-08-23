use std::sync::Mutex;

use async_trait::async_trait;
use chrono::TimeZone;
use ryframe_kernel::DataScope;

use super::*;
use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode, ports::system::NoticeTransaction,
};

struct FakePersistence {
    calls: Arc<Mutex<Vec<&'static str>>>,
    record: NoticeRecord,
}

struct FakeTransaction {
    calls: Arc<Mutex<Vec<&'static str>>>,
    record: NoticeRecord,
}

#[async_trait]
impl NoticePersistencePort for FakePersistence {
    async fn find_by_id(&self, _tenant_id: &str, _id: i64) -> AppResult<Option<NoticeRecord>> {
        unreachable!("本测试不读取详情")
    }

    async fn find_by_page(
        &self,
        _tenant_id: &str,
        _page: ValidatedPageQuery,
        _filter: NoticeFilter<'_>,
    ) -> AppResult<PageResult<NoticeRecord>> {
        unreachable!("本测试不读取列表")
    }

    async fn begin(&self) -> AppResult<Box<dyn NoticeTransaction>> {
        self.calls.lock().expect("调用记录锁应可用").push("begin");
        let transaction = FakeTransaction {
            calls: Arc::clone(&self.calls),
            record: self.record.clone(),
        };
        Ok(Box::new(transaction) as Box<dyn NoticeTransaction>)
    }
}

#[async_trait]
impl NoticeTransaction for FakeTransaction {
    async fn find_by_id_for_update(
        &self,
        _tenant_id: &str,
        _id: i64,
    ) -> AppResult<Option<NoticeRecord>> {
        self.calls.lock().expect("调用记录锁应可用").push("find");
        Ok(Some(self.record.clone()))
    }

    async fn insert(&self, _tenant_id: &str, record: NoticeRecord) -> AppResult<NoticeRecord> {
        Ok(record)
    }

    async fn update(&self, _tenant_id: &str, record: NoticeRecord) -> AppResult<NoticeRecord> {
        self.calls.lock().expect("调用记录锁应可用").push("update");
        Ok(record)
    }

    async fn delete(&self, _tenant_id: &str, _id: i64) -> AppResult<()> {
        Ok(())
    }
}

#[async_trait]
impl PersistenceTransaction for FakeTransaction {
    async fn commit(self: Box<Self>, _audit_mode: TransactionAuditMode) -> AppResult<()> {
        self.calls.lock().expect("调用记录锁应可用").push("commit");
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.calls
            .lock()
            .expect("调用记录锁应可用")
            .push("rollback");
        Ok(())
    }
}

#[tokio::test]
async fn update_locks_row_inside_application_owned_transaction() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let timestamp = Utc
        .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
        .single()
        .expect("测试时间应有效");
    let persistence = Arc::new(FakePersistence {
        calls: Arc::clone(&calls),
        record: NoticeRecord {
            id: 8,
            title: "旧通知".into(),
            content: "旧内容".into(),
            notice_type: Some("1".into()),
            status: "1".into(),
            created_by: Some(1),
            created_at: timestamp,
            updated_at: timestamp,
        },
    });
    let service = NoticeService::new(persistence);
    let actor = ActorContext {
        user_id: 1,
        tenant_id: "tenant-a".into(),
        username: "tester".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::SelfOnly,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: false,
    };

    let updated = service
        .update(&actor, 8, "新通知", "新内容", Some("2"), "0".into())
        .await
        .expect("通知更新应成功");

    assert_eq!(updated.title, "新通知");
    assert_eq!(updated.content_markdown, "新内容");
    assert_eq!(updated.notice_type.as_deref(), Some("2"));
    assert_eq!(
        *calls.lock().expect("调用记录锁应可用"),
        ["begin", "find", "update", "commit"]
    );
}
