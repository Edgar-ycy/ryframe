use std::sync::Mutex;

use async_trait::async_trait;
use ryframe_kernel::DataScope;

use super::*;
use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode, ports::system::ConfigTransaction,
};

struct FakePersistence {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

struct FakeTransaction {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait]
impl ConfigPersistencePort for FakePersistence {
    async fn find_by_page(
        &self,
        _tenant_id: &str,
        _page: ValidatedPageQuery,
        _filter: ConfigFilter<'_>,
    ) -> AppResult<PageResult<ConfigRecord>> {
        unreachable!("本测试不读取列表")
    }

    async fn find_export_batch(
        &self,
        _tenant_id: &str,
        _filter: ConfigFilter<'_>,
        _window: ExportCursorWindow,
    ) -> AppResult<Vec<ConfigRecord>> {
        unreachable!("本测试不执行导出")
    }

    async fn find_by_id(&self, _tenant_id: &str, _id: i64) -> AppResult<Option<ConfigRecord>> {
        unreachable!("本测试不读取详情")
    }

    async fn find_by_key(&self, _tenant_id: &str, _key: &str) -> AppResult<Option<ConfigRecord>> {
        unreachable!("本测试不读取键值")
    }

    async fn find_namespace_version(&self, _tenant_id: &str, _namespace: &str) -> AppResult<i64> {
        unreachable!("本测试不读取缓存版本")
    }

    async fn begin(&self) -> AppResult<Box<dyn ConfigTransaction>> {
        self.calls.lock().expect("调用记录锁应可用").push("begin");
        let transaction = FakeTransaction {
            calls: Arc::clone(&self.calls),
        };
        Ok(Box::new(transaction) as Box<dyn ConfigTransaction>)
    }
}

#[async_trait]
impl ConfigTransaction for FakeTransaction {
    async fn lock_configuration(&self, _tenant_id: &str) -> AppResult<()> {
        unreachable!("本测试不锁定配置")
    }

    async fn find_by_key_for_update(
        &self,
        _tenant_id: &str,
        _key: &str,
    ) -> AppResult<Option<ConfigRecord>> {
        unreachable!("本测试不读取键值")
    }

    async fn find_by_id_for_update(
        &self,
        _tenant_id: &str,
        _id: i64,
    ) -> AppResult<Option<ConfigRecord>> {
        unreachable!("本测试不读取详情")
    }

    async fn insert(&self, _tenant_id: &str, _record: ConfigRecord) -> AppResult<ConfigRecord> {
        unreachable!("本测试不新增配置")
    }

    async fn update(&self, _tenant_id: &str, _record: ConfigRecord) -> AppResult<ConfigRecord> {
        unreachable!("本测试不更新配置")
    }

    async fn delete(&self, _tenant_id: &str, _id: i64) -> AppResult<()> {
        unreachable!("本测试不删除配置")
    }

    async fn record_namespace_change(&self, _tenant_id: &str, _namespace: &str) -> AppResult<i64> {
        self.calls
            .lock()
            .expect("调用记录锁应可用")
            .push("namespace");
        Ok(8)
    }

    async fn increment_configuration_version(&self, _tenant_id: &str) -> AppResult<()> {
        unreachable!("本测试不递增配置版本")
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
async fn cache_clear_commits_authoritative_version_first() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let service = ConfigService::new(
        Arc::new(FakePersistence {
            calls: Arc::clone(&calls),
        }),
        AuthorizationCache::disabled(),
    );
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

    assert_eq!(service.clear_cache(&actor).await.expect("清理应成功"), 1);
    assert_eq!(
        *calls.lock().expect("调用记录锁应可用"),
        ["begin", "namespace", "commit"]
    );
}
