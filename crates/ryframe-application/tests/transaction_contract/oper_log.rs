use std::sync::Mutex;

use async_trait::async_trait;
use ryframe_kernel::DataScope;

use super::*;
use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode, ports::system::OperLogTransaction,
};

struct FakePersistence {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

struct FakeTransaction {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait]
impl OperLogPersistencePort for FakePersistence {
    async fn insert(&self, _tenant_id: &str, _record: OperLogRecord) -> AppResult<()> {
        unreachable!("本测试不写入日志")
    }

    async fn find_by_page(
        &self,
        _tenant_id: &str,
        _page: ValidatedPageQuery,
        _filter: OperLogFilter<'_>,
        _data_scope: &DataScopeContext,
    ) -> AppResult<PageResult<OperLogRecord>> {
        unreachable!("本测试不读取列表")
    }

    async fn find_export_batch(
        &self,
        _tenant_id: &str,
        _filter: OperLogFilter<'_>,
        _data_scope: &DataScopeContext,
        _window: ExportCursorWindow,
    ) -> AppResult<Vec<OperLogRecord>> {
        unreachable!("本测试不执行导出")
    }

    async fn begin(&self) -> AppResult<Box<dyn OperLogTransaction>> {
        self.calls.lock().expect("调用记录锁应可用").push("begin");
        let transaction = FakeTransaction {
            calls: Arc::clone(&self.calls),
        };
        Ok(Box::new(transaction) as Box<dyn OperLogTransaction>)
    }
}

#[async_trait]
impl OperLogTransaction for FakeTransaction {
    async fn clean(&self, _tenant_id: &str) -> AppResult<u64> {
        self.calls.lock().expect("调用记录锁应可用").push("clean");
        Ok(4)
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
async fn clean_is_committed_by_application_use_case() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let service = OperLogService::new(Arc::new(FakePersistence {
        calls: Arc::clone(&calls),
    }));
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

    assert_eq!(service.clean(&actor).await.expect("清理应成功"), 4);
    assert_eq!(
        *calls.lock().expect("调用记录锁应可用"),
        ["begin", "clean", "commit"]
    );
}

#[test]
fn operation_status_keeps_persisted_codes() {
    assert_eq!(OperLogStatus::Success.as_str(), "1");
    assert_eq!(OperLogStatus::Failure.as_str(), "0");
}
