use std::sync::Arc;

use chrono::Utc;
use ryframe_application::{AuthorizationCache, MessagingPolicy, ports::system::*, system::*};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_kernel::*;

mod config {
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

        async fn find_by_key(
            &self,
            _tenant_id: &str,
            _key: &str,
        ) -> AppResult<Option<ConfigRecord>> {
            unreachable!("本测试不读取键值")
        }

        async fn find_namespace_version(
            &self,
            _tenant_id: &str,
            _namespace: &str,
        ) -> AppResult<i64> {
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

        async fn record_namespace_change(
            &self,
            _tenant_id: &str,
            _namespace: &str,
        ) -> AppResult<i64> {
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
}

mod login_info {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use ryframe_kernel::DataScope;

    use super::*;
    use ryframe_application::{
        PersistenceTransaction, TransactionAuditMode, ports::system::LoginInfoTransaction,
    };

    struct FakePersistence {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl LoginInfoPersistencePort for FakePersistence {
        async fn insert(&self, _tenant_id: &str, _record: LoginInfoRecord) -> AppResult<()> {
            unreachable!("本测试不写入日志")
        }

        async fn find_by_page(
            &self,
            _tenant_id: &str,
            _page: ValidatedPageQuery,
            _filter: LoginInfoFilter<'_>,
            _data_scope: &DataScopeContext,
        ) -> AppResult<PageResult<LoginInfoRecord>> {
            unreachable!("本测试不读取列表")
        }

        async fn find_export_batch(
            &self,
            _tenant_id: &str,
            _filter: LoginInfoFilter<'_>,
            _data_scope: &DataScopeContext,
            _window: ExportCursorWindow,
        ) -> AppResult<Vec<LoginInfoRecord>> {
            unreachable!("本测试不执行导出")
        }

        async fn begin(&self) -> AppResult<Box<dyn LoginInfoTransaction>> {
            self.calls.lock().expect("调用记录锁应可用").push("begin");
            let transaction = FakeTransaction {
                calls: Arc::clone(&self.calls),
            };
            Ok(Box::new(transaction) as Box<dyn LoginInfoTransaction>)
        }
    }

    #[async_trait]
    impl LoginInfoTransaction for FakeTransaction {
        async fn clean(&self, _tenant_id: &str) -> AppResult<u64> {
            self.calls.lock().expect("调用记录锁应可用").push("clean");
            Ok(3)
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
        let service = LoginInfoService::new(Arc::new(FakePersistence {
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

        assert_eq!(service.clean(&actor).await.expect("清理应成功"), 3);
        assert_eq!(
            *calls.lock().expect("调用记录锁应可用"),
            ["begin", "clean", "commit"]
        );
    }

    #[test]
    fn login_status_keeps_persisted_codes() {
        assert_eq!(LoginStatus::Success.as_str(), "1");
        assert_eq!(LoginStatus::Failure.as_str(), "0");
    }
}

mod notice {
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
}

mod oper_log {
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
}

mod post {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::TimeZone;
    use ryframe_kernel::DataScope;

    use super::*;
    use ryframe_application::{
        PersistenceTransaction, TransactionAuditMode, ports::system::PostTransaction,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Call {
        Begin,
        Lock,
        FindByCode,
        FindById,
        Update,
        IncrementVersion,
        Commit(TransactionAuditMode),
        Rollback,
    }

    struct FakePersistence {
        calls: Arc<Mutex<Vec<Call>>>,
        record: PostRecord,
        duplicate_code: bool,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<Call>>>,
        record: PostRecord,
        duplicate_code: bool,
    }

    #[async_trait]
    impl PostPersistencePort for FakePersistence {
        async fn find_by_id(&self, _tenant_id: &str, _id: i64) -> AppResult<Option<PostRecord>> {
            unreachable!("本测试不读取详情")
        }

        async fn find_by_page(
            &self,
            _tenant_id: &str,
            _page: ValidatedPageQuery,
            _filter: PostFilter<'_>,
        ) -> AppResult<PageResult<PostRecord>> {
            unreachable!("本测试不读取列表")
        }

        async fn find_export_batch(
            &self,
            _tenant_id: &str,
            _filter: PostFilter<'_>,
            _window: ExportCursorWindow,
        ) -> AppResult<Vec<PostRecord>> {
            unreachable!("本测试不执行导出")
        }

        async fn begin(&self) -> AppResult<Box<dyn PostTransaction>> {
            self.calls
                .lock()
                .expect("调用记录锁应可用")
                .push(Call::Begin);
            let transaction = FakeTransaction {
                calls: Arc::clone(&self.calls),
                record: self.record.clone(),
                duplicate_code: self.duplicate_code,
            };
            Ok(Box::new(transaction) as Box<dyn PostTransaction>)
        }
    }

    #[async_trait]
    impl PostTransaction for FakeTransaction {
        async fn lock_configuration(&self, _tenant_id: &str) -> AppResult<()> {
            self.calls
                .lock()
                .expect("调用记录锁应可用")
                .push(Call::Lock);
            Ok(())
        }

        async fn find_by_code_for_update(
            &self,
            _tenant_id: &str,
            _code: &str,
        ) -> AppResult<Option<PostRecord>> {
            self.calls
                .lock()
                .expect("调用记录锁应可用")
                .push(Call::FindByCode);
            Ok(self.duplicate_code.then(|| self.record.clone()))
        }

        async fn find_by_id_for_update(
            &self,
            _tenant_id: &str,
            _id: i64,
        ) -> AppResult<Option<PostRecord>> {
            self.calls
                .lock()
                .expect("调用记录锁应可用")
                .push(Call::FindById);
            Ok(Some(self.record.clone()))
        }

        async fn insert(&self, _tenant_id: &str, record: PostRecord) -> AppResult<PostRecord> {
            Ok(record)
        }

        async fn update(&self, _tenant_id: &str, record: PostRecord) -> AppResult<PostRecord> {
            self.calls
                .lock()
                .expect("调用记录锁应可用")
                .push(Call::Update);
            Ok(record)
        }

        async fn delete(&self, _tenant_id: &str, _id: i64) -> AppResult<()> {
            Ok(())
        }

        async fn increment_configuration_version(&self, _tenant_id: &str) -> AppResult<()> {
            self.calls
                .lock()
                .expect("调用记录锁应可用")
                .push(Call::IncrementVersion);
            Ok(())
        }
    }

    #[async_trait]
    impl PersistenceTransaction for FakeTransaction {
        async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {
            self.calls
                .lock()
                .expect("调用记录锁应可用")
                .push(Call::Commit(audit_mode));
            Ok(())
        }

        async fn rollback(self: Box<Self>) -> AppResult<()> {
            self.calls
                .lock()
                .expect("调用记录锁应可用")
                .push(Call::Rollback);
            Ok(())
        }
    }

    fn record(timestamp: chrono::DateTime<Utc>) -> PostRecord {
        PostRecord {
            id: 7,
            name: "旧岗位".into(),
            code: "old".into(),
            sort: 1,
            status: "1".into(),
            remark: None,
            created_at: timestamp,
            updated_at: timestamp,
        }
    }

    fn actor() -> ActorContext {
        ActorContext {
            user_id: 1,
            tenant_id: "tenant-a".into(),
            username: "tester".into(),
            dept_id: None,
            dept_path: None,
            data_scope: DataScope::SelfOnly,
            custom_dept_ids: Vec::new(),
            include_self: true,
            is_super_admin: false,
        }
    }

    fn fixed_id() -> AppResult<i64> {
        Ok(8)
    }

    #[tokio::test]
    async fn update_owns_transaction_order() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
            .single()
            .expect("测试时间应有效");
        let persistence = Arc::new(FakePersistence {
            calls: Arc::clone(&calls),
            record: record(timestamp),
            duplicate_code: false,
        });
        let service = PostService::new(persistence);
        let actor = actor();

        let updated = service
            .update(&actor, 7, "新岗位", 2, "0".into())
            .await
            .expect("岗位更新应成功");

        assert_eq!(updated.name, "新岗位");
        assert_eq!(updated.sort, 2);
        assert_eq!(updated.status, "0");
        assert_eq!(
            *calls.lock().expect("调用记录锁应可用"),
            [
                Call::Begin,
                Call::Lock,
                Call::FindById,
                Call::Update,
                Call::IncrementVersion,
                Call::Commit(TransactionAuditMode::CurrentRequest),
            ]
        );
    }

    #[tokio::test]
    async fn create_rolls_back_on_duplicate_code() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
            .single()
            .expect("测试时间应有效");
        let persistence = Arc::new(FakePersistence {
            calls: Arc::clone(&calls),
            record: record(timestamp),
            duplicate_code: true,
        });
        let service = PostService::new(persistence);
        ryframe_application::install_id_generator(fixed_id).expect("测试 ID 生成器应安装成功");

        let error = service
            .create(&actor(), "新岗位", "old", 2)
            .await
            .expect_err("重复岗位编码应失败");

        assert!(matches!(error, AppError::Conflict(_)));
        assert_eq!(
            *calls.lock().expect("调用记录锁应可用"),
            [Call::Begin, Call::Lock, Call::FindByCode, Call::Rollback]
        );
    }
}

mod websocket_ticket {
    use std::{collections::HashMap, sync::Arc};

    use ryframe_kernel::{ActorContext, DataScope};
    use tokio::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemoryTicketStore {
        values: Mutex<HashMap<String, String>>,
    }

    impl WebSocketTicketStore for MemoryTicketStore {
        fn put(
            &self,
            key: String,
            value: String,
            _ttl_secs: u64,
        ) -> WebSocketTicketStoreFuture<'_, ()> {
            Box::pin(async move {
                self.values.lock().await.insert(key, value);
                Ok(())
            })
        }

        fn take<'a>(&'a self, key: &'a str) -> WebSocketTicketStoreFuture<'a, Option<String>> {
            Box::pin(async move { Ok(self.values.lock().await.remove(key)) })
        }
    }

    #[tokio::test]
    async fn issued_ticket_can_only_be_consumed_once() {
        let store = Arc::new(MemoryTicketStore::default());
        let service = WebSocketTicketService::new(
            Some(store),
            MessagingPolicy::new(true, 60, 7, 100).expect("策略应有效"),
        );
        let principal = RequestPrincipal {
            actor: ActorContext {
                user_id: 42,
                tenant_id: "tenant-a".into(),
                username: "tester".into(),
                dept_id: None,
                dept_path: None,
                data_scope: DataScope::SelfOnly,
                custom_dept_ids: Vec::new(),
                include_self: true,
                is_super_admin: false,
            },
            tenant_authorization_epoch: 0,
            preferred_locale: None,
            roles: Vec::new(),
            role_ids: Vec::new(),
            permissions: Vec::new(),
            tenant_request_limit_per_minute: 0,
        };
        let claims = Claims {
            sub: "42".into(),
            tenant_id: "tenant-a".into(),
            tenant_session_version: 3,
            user_authorization_version: 4,
            username: "tester".into(),
            token_type: "access".into(),
            sid: "session-a".into(),
            jti: "token-a".into(),
            iat: 1,
            exp: 2,
        };

        let grant = service
            .issue(&principal, &claims, "en-GB")
            .await
            .expect("票据应签发成功");
        let consumed = service
            .consume(&grant.ticket)
            .await
            .expect("首次消费应成功");
        assert_eq!(consumed.user_id, 42);
        assert_eq!(consumed.locale, "en-US");
        assert!(service.consume(&grant.ticket).await.is_err());
    }
}
