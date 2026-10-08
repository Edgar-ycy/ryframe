use std::sync::Mutex;

use async_trait::async_trait;
use ryframe_kernel::DataScope;

use super::*;
use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode, ports::system::ConfigTransaction,
};

struct FakePersistence {
    calls: Arc<Mutex<Vec<&'static str>>>,
    reads: Arc<Mutex<Vec<(String, String)>>>,
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
        _window: ExportCursorWindow<'_>,
    ) -> AppResult<Vec<ConfigRecord>> {
        unreachable!("本测试不执行导出")
    }

    async fn find_by_id(&self, _tenant_id: &str, _id: i64) -> AppResult<Option<ConfigRecord>> {
        unreachable!("本测试不读取详情")
    }

    async fn find_by_key(&self, tenant_id: &str, key: &str) -> AppResult<Option<ConfigRecord>> {
        self.reads
            .lock()
            .expect("读取记录锁应可用")
            .push((tenant_id.into(), key.into()));
        if tenant_id == "empty" {
            return Ok(None);
        }
        if tenant_id == "unavailable" {
            return Err(AppError::ServiceUnavailable("配置不可用".into()));
        }
        Ok(Some(ConfigRecord {
            id: 1,
            name: "界面配置".into(),
            key: key.into(),
            value: format!("{tenant_id}:{key}"),
            portable: true,
            remark: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }))
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
            reads: Arc::default(),
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

#[tokio::test]
async fn shell_settings_only_read_fixed_keys_in_the_authenticated_tenant() {
    let reads = Arc::new(Mutex::new(Vec::new()));
    let service = ConfigService::new(
        Arc::new(FakePersistence {
            calls: Arc::default(),
            reads: reads.clone(),
        }),
        AuthorizationCache::disabled(),
    );
    let mut actor = ActorContext {
        user_id: 1,
        tenant_id: "tenant-a".into(),
        username: "ordinary".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::SelfOnly,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: false,
    };
    for tenant in ["tenant-a", "tenant-b", "empty"] {
        actor.tenant_id = tenant.into();
        let settings = service
            .shell_settings(&actor)
            .await
            .expect("普通身份应能读取界面配置");
        let expected = |key| (tenant != "empty").then(|| format!("{tenant}:{key}"));
        assert_eq!(settings.side_theme, expected("sys.index.sideTheme"));
        assert_eq!(settings.skin_name, expected("sys.index.skinName"));
    }
    let expected: Vec<_> = ["tenant-a", "tenant-b", "empty"]
        .into_iter()
        .flat_map(|tenant| {
            ["sys.index.sideTheme", "sys.index.skinName"]
                .map(|key| (tenant.to_owned(), key.to_owned()))
        })
        .collect();
    assert_eq!(*reads.lock().expect("读取记录锁应可用"), expected);
    actor.tenant_id = "unavailable".into();
    assert!(service.shell_settings(&actor).await.is_err());
    actor.tenant_id.clear();
    assert!(service.shell_settings(&actor).await.is_err());
    assert_eq!(reads.lock().expect("读取记录锁应可用").len(), 7);
}
