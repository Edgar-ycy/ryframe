use std::sync::Arc;

use chrono::Utc;
use ryframe_application::{AuthorizationCache, MessagingPolicy, ports::system::*, system::*};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_kernel::*;

mod config {
    use std::sync::Mutex;

    use ryframe_kernel::DataScope;

    use super::*;
    use ryframe_application::{
        ControlTransaction, PersistenceFuture, ports::system::ConfigTransaction,
    };

    struct FakePersistence {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl ConfigPersistencePort for FakePersistence {
        fn find_by_page<'a>(
            &'a self,
            _tenant_id: &'a str,
            _page: ValidatedPageQuery,
            _filter: ConfigFilter<'a>,
        ) -> PersistenceFuture<'a, PageResult<ConfigRecord>> {
            Box::pin(async { unreachable!("本测试不读取列表") })
        }

        fn find_export_batch<'a>(
            &'a self,
            _tenant_id: &'a str,
            _filter: ConfigFilter<'a>,
            _window: ExportCursorWindow,
        ) -> PersistenceFuture<'a, Vec<ConfigRecord>> {
            Box::pin(async { unreachable!("本测试不执行导出") })
        }

        fn find_by_id<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: i64,
        ) -> PersistenceFuture<'a, Option<ConfigRecord>> {
            Box::pin(async { unreachable!("本测试不读取详情") })
        }

        fn find_by_key<'a>(
            &'a self,
            _tenant_id: &'a str,
            _key: &'a str,
        ) -> PersistenceFuture<'a, Option<ConfigRecord>> {
            Box::pin(async { unreachable!("本测试不读取键值") })
        }

        fn find_namespace_version<'a>(
            &'a self,
            _tenant_id: &'a str,
            _namespace: &'a str,
        ) -> PersistenceFuture<'a, i64> {
            Box::pin(async { unreachable!("本测试不读取缓存版本") })
        }

        fn begin(&self) -> PersistenceFuture<'_, Box<dyn ConfigTransaction>> {
            self.calls.lock().expect("调用记录锁应可用").push("begin");
            let transaction = FakeTransaction {
                calls: Arc::clone(&self.calls),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn ConfigTransaction>) })
        }
    }

    impl ConfigTransaction for FakeTransaction {
        fn lock_configuration<'a>(&'a self, _tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
            Box::pin(async { unreachable!("本测试不锁定配置") })
        }

        fn find_by_key_for_update<'a>(
            &'a self,
            _tenant_id: &'a str,
            _key: &'a str,
        ) -> PersistenceFuture<'a, Option<ConfigRecord>> {
            Box::pin(async { unreachable!("本测试不读取键值") })
        }

        fn find_by_id_for_update<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: i64,
        ) -> PersistenceFuture<'a, Option<ConfigRecord>> {
            Box::pin(async { unreachable!("本测试不读取详情") })
        }

        fn insert<'a>(
            &'a self,
            _tenant_id: &'a str,
            _record: ConfigRecord,
        ) -> PersistenceFuture<'a, ConfigRecord> {
            Box::pin(async { unreachable!("本测试不新增配置") })
        }

        fn update<'a>(
            &'a self,
            _tenant_id: &'a str,
            _record: ConfigRecord,
        ) -> PersistenceFuture<'a, ConfigRecord> {
            Box::pin(async { unreachable!("本测试不更新配置") })
        }

        fn delete<'a>(&'a self, _tenant_id: &'a str, _id: i64) -> PersistenceFuture<'a, ()> {
            Box::pin(async { unreachable!("本测试不删除配置") })
        }

        fn record_namespace_change<'a>(
            &'a self,
            _tenant_id: &'a str,
            _namespace: &'a str,
        ) -> PersistenceFuture<'a, i64> {
            self.calls
                .lock()
                .expect("调用记录锁应可用")
                .push("namespace");
            Box::pin(async { Ok(8) })
        }

        fn increment_configuration_version<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> PersistenceFuture<'a, ()> {
            Box::pin(async { unreachable!("本测试不递增配置版本") })
        }
    }

    impl ControlTransaction for FakeTransaction {
        fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("commit");
            Box::pin(async { Ok(()) })
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

    use ryframe_kernel::DataScope;

    use super::*;
    use ryframe_application::{
        ControlTransaction, PersistenceFuture, ports::system::LoginInfoTransaction,
    };

    struct FakePersistence {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl LoginInfoPersistencePort for FakePersistence {
        fn insert<'a>(
            &'a self,
            _tenant_id: &'a str,
            _record: LoginInfoRecord,
        ) -> PersistenceFuture<'a, ()> {
            Box::pin(async { unreachable!("本测试不写入日志") })
        }

        fn find_by_page<'a>(
            &'a self,
            _tenant_id: &'a str,
            _page: ValidatedPageQuery,
            _filter: LoginInfoFilter<'a>,
            _data_scope: &'a ryframe_kernel::DataScopeContext,
        ) -> PersistenceFuture<'a, PageResult<LoginInfoRecord>> {
            Box::pin(async { unreachable!("本测试不读取列表") })
        }

        fn find_export_batch<'a>(
            &'a self,
            _tenant_id: &'a str,
            _filter: LoginInfoFilter<'a>,
            _data_scope: &'a ryframe_kernel::DataScopeContext,
            _window: ExportCursorWindow,
        ) -> PersistenceFuture<'a, Vec<LoginInfoRecord>> {
            Box::pin(async { unreachable!("本测试不执行导出") })
        }

        fn begin(&self) -> PersistenceFuture<'_, Box<dyn LoginInfoTransaction>> {
            self.calls.lock().expect("调用记录锁应可用").push("begin");
            let transaction = FakeTransaction {
                calls: Arc::clone(&self.calls),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn LoginInfoTransaction>) })
        }
    }

    impl LoginInfoTransaction for FakeTransaction {
        fn clean<'a>(&'a self, _tenant_id: &'a str) -> PersistenceFuture<'a, u64> {
            self.calls.lock().expect("调用记录锁应可用").push("clean");
            Box::pin(async { Ok(3) })
        }
    }

    impl ControlTransaction for FakeTransaction {
        fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("commit");
            Box::pin(async { Ok(()) })
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

    use chrono::TimeZone;
    use ryframe_kernel::DataScope;

    use super::*;
    use ryframe_application::{
        ControlTransaction, PersistenceFuture, ports::system::NoticeTransaction,
    };

    struct FakePersistence {
        calls: Arc<Mutex<Vec<&'static str>>>,
        record: NoticeRecord,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<&'static str>>>,
        record: NoticeRecord,
    }

    impl NoticePersistencePort for FakePersistence {
        fn find_by_id<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: i64,
        ) -> PersistenceFuture<'a, Option<NoticeRecord>> {
            Box::pin(async { unreachable!("本测试不读取详情") })
        }

        fn find_by_page<'a>(
            &'a self,
            _tenant_id: &'a str,
            _page: ValidatedPageQuery,
            _filter: NoticeFilter<'a>,
        ) -> PersistenceFuture<'a, PageResult<NoticeRecord>> {
            Box::pin(async { unreachable!("本测试不读取列表") })
        }

        fn begin(&self) -> PersistenceFuture<'_, Box<dyn NoticeTransaction>> {
            self.calls.lock().expect("调用记录锁应可用").push("begin");
            let transaction = FakeTransaction {
                calls: Arc::clone(&self.calls),
                record: self.record.clone(),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn NoticeTransaction>) })
        }
    }

    impl NoticeTransaction for FakeTransaction {
        fn find_by_id_for_update<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: i64,
        ) -> PersistenceFuture<'a, Option<NoticeRecord>> {
            self.calls.lock().expect("调用记录锁应可用").push("find");
            let record = self.record.clone();
            Box::pin(async move { Ok(Some(record)) })
        }

        fn insert<'a>(
            &'a self,
            _tenant_id: &'a str,
            record: NoticeRecord,
        ) -> PersistenceFuture<'a, NoticeRecord> {
            Box::pin(async move { Ok(record) })
        }

        fn update<'a>(
            &'a self,
            _tenant_id: &'a str,
            record: NoticeRecord,
        ) -> PersistenceFuture<'a, NoticeRecord> {
            self.calls.lock().expect("调用记录锁应可用").push("update");
            Box::pin(async move { Ok(record) })
        }

        fn delete<'a>(&'a self, _tenant_id: &'a str, _id: i64) -> PersistenceFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    impl ControlTransaction for FakeTransaction {
        fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("commit");
            Box::pin(async { Ok(()) })
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

    use ryframe_kernel::DataScope;

    use super::*;
    use ryframe_application::{
        ControlTransaction, PersistenceFuture, ports::system::OperLogTransaction,
    };

    struct FakePersistence {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl OperLogPersistencePort for FakePersistence {
        fn insert<'a>(
            &'a self,
            _tenant_id: &'a str,
            _record: OperLogRecord,
        ) -> PersistenceFuture<'a, ()> {
            Box::pin(async { unreachable!("本测试不写入日志") })
        }

        fn find_by_page<'a>(
            &'a self,
            _tenant_id: &'a str,
            _page: ValidatedPageQuery,
            _filter: OperLogFilter<'a>,
            _data_scope: &'a ryframe_kernel::DataScopeContext,
        ) -> PersistenceFuture<'a, PageResult<OperLogRecord>> {
            Box::pin(async { unreachable!("本测试不读取列表") })
        }

        fn find_export_batch<'a>(
            &'a self,
            _tenant_id: &'a str,
            _filter: OperLogFilter<'a>,
            _data_scope: &'a ryframe_kernel::DataScopeContext,
            _window: ExportCursorWindow,
        ) -> PersistenceFuture<'a, Vec<OperLogRecord>> {
            Box::pin(async { unreachable!("本测试不执行导出") })
        }

        fn begin(&self) -> PersistenceFuture<'_, Box<dyn OperLogTransaction>> {
            self.calls.lock().expect("调用记录锁应可用").push("begin");
            let transaction = FakeTransaction {
                calls: Arc::clone(&self.calls),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn OperLogTransaction>) })
        }
    }

    impl OperLogTransaction for FakeTransaction {
        fn clean<'a>(&'a self, _tenant_id: &'a str) -> PersistenceFuture<'a, u64> {
            self.calls.lock().expect("调用记录锁应可用").push("clean");
            Box::pin(async { Ok(4) })
        }
    }

    impl ControlTransaction for FakeTransaction {
        fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("commit");
            Box::pin(async { Ok(()) })
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

    use chrono::TimeZone;
    use ryframe_kernel::DataScope;

    use super::*;
    use ryframe_application::{
        ControlTransaction, PersistenceFuture, ports::system::PostTransaction,
    };

    struct FakePersistence {
        calls: Arc<Mutex<Vec<&'static str>>>,
        record: PostRecord,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<&'static str>>>,
        record: PostRecord,
    }

    impl PostPersistencePort for FakePersistence {
        fn find_by_id<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: i64,
        ) -> PersistenceFuture<'a, Option<PostRecord>> {
            Box::pin(async { unreachable!("本测试不读取详情") })
        }

        fn find_by_page<'a>(
            &'a self,
            _tenant_id: &'a str,
            _page: ValidatedPageQuery,
            _filter: PostFilter<'a>,
        ) -> PersistenceFuture<'a, PageResult<PostRecord>> {
            Box::pin(async { unreachable!("本测试不读取列表") })
        }

        fn find_export_batch<'a>(
            &'a self,
            _tenant_id: &'a str,
            _filter: PostFilter<'a>,
            _window: ExportCursorWindow,
        ) -> PersistenceFuture<'a, Vec<PostRecord>> {
            Box::pin(async { unreachable!("本测试不执行导出") })
        }

        fn begin(&self) -> PersistenceFuture<'_, Box<dyn PostTransaction>> {
            self.calls.lock().expect("调用记录锁应可用").push("begin");
            let transaction = FakeTransaction {
                calls: Arc::clone(&self.calls),
                record: self.record.clone(),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn PostTransaction>) })
        }
    }

    impl PostTransaction for FakeTransaction {
        fn lock_configuration<'a>(&'a self, _tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("lock");
            Box::pin(async { Ok(()) })
        }

        fn find_by_code_for_update<'a>(
            &'a self,
            _tenant_id: &'a str,
            _code: &'a str,
        ) -> PersistenceFuture<'a, Option<PostRecord>> {
            Box::pin(async { Ok(None) })
        }

        fn find_by_id_for_update<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: i64,
        ) -> PersistenceFuture<'a, Option<PostRecord>> {
            self.calls.lock().expect("调用记录锁应可用").push("find");
            let record = self.record.clone();
            Box::pin(async move { Ok(Some(record)) })
        }

        fn insert<'a>(
            &'a self,
            _tenant_id: &'a str,
            record: PostRecord,
        ) -> PersistenceFuture<'a, PostRecord> {
            Box::pin(async move { Ok(record) })
        }

        fn update<'a>(
            &'a self,
            _tenant_id: &'a str,
            record: PostRecord,
        ) -> PersistenceFuture<'a, PostRecord> {
            self.calls.lock().expect("调用记录锁应可用").push("update");
            Box::pin(async move { Ok(record) })
        }

        fn delete<'a>(&'a self, _tenant_id: &'a str, _id: i64) -> PersistenceFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }

        fn increment_configuration_version<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> PersistenceFuture<'a, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("version");
            Box::pin(async { Ok(()) })
        }
    }

    impl ControlTransaction for FakeTransaction {
        fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
            self.calls.lock().expect("调用记录锁应可用").push("commit");
            Box::pin(async { Ok(()) })
        }
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
            record: PostRecord {
                id: 7,
                name: "旧岗位".into(),
                code: "old".into(),
                sort: 1,
                status: "1".into(),
                remark: None,
                created_at: timestamp,
                updated_at: timestamp,
            },
        });
        let service = PostService::new(persistence);
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
            .update(&actor, 7, "新岗位", 2, "0".into())
            .await
            .expect("岗位更新应成功");

        assert_eq!(updated.name, "新岗位");
        assert_eq!(updated.sort, 2);
        assert_eq!(updated.status, "0");
        assert_eq!(
            *calls.lock().expect("调用记录锁应可用"),
            ["begin", "lock", "find", "update", "version", "commit"]
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
