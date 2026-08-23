use std::sync::Arc;

use chrono::Utc;
use ryframe_application::{AuthorizationCache, MessagingPolicy, ports::system::*, system::*};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_kernel::*;

mod transaction_completion {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use ryframe_application::{PersistenceTransaction, TransactionAuditMode, complete_transaction};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Call {
        Commit(TransactionAuditMode),
        Rollback,
    }

    struct FakeTransaction {
        calls: Arc<Mutex<Vec<Call>>>,
        rollback_fails: bool,
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
            if self.rollback_fails {
                Err(AppError::Internal("模拟回滚失败".into()))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn success_forwards_the_requested_audit_mode() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let result = complete_transaction(
            Box::new(FakeTransaction {
                calls: Arc::clone(&calls),
                rollback_fails: false,
            }),
            Ok(7),
            TransactionAuditMode::Skip,
        )
        .await
        .expect("事务应提交成功");

        assert_eq!(result, 7);
        assert_eq!(
            *calls.lock().expect("调用记录锁应可用"),
            [Call::Commit(TransactionAuditMode::Skip)]
        );
    }

    #[tokio::test]
    async fn failure_rolls_back_without_hiding_the_operation_error() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let error = complete_transaction(
            Box::new(FakeTransaction {
                calls: Arc::clone(&calls),
                rollback_fails: true,
            }),
            Err::<(), _>(AppError::Conflict("业务冲突".into())),
            TransactionAuditMode::CurrentRequest,
        )
        .await
        .expect_err("业务操作失败时应返回错误");

        assert!(matches!(error, AppError::Conflict(message) if message == "业务冲突"));
        assert_eq!(*calls.lock().expect("调用记录锁应可用"), [Call::Rollback]);
    }
}

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
    use chrono::TimeZone;
    use ryframe_kernel::DataScope;

    use super::*;
    use ryframe_application::{
        TransactionAuditMode,
        generated::post::{
            CreatePostCommand, PostCall, PostFakePersistence, PostListParams, PostPersistencePort,
            PostRecord, PostService, PostTransactionState, UpdatePostCommand,
        },
    };

    fn record(timestamp: chrono::DateTime<Utc>) -> PostRecord {
        PostRecord {
            id: 7,
            tenant_id: "tenant-a".into(),
            name: "旧岗位".into(),
            code: "old".into(),
            sort: 1,
            status: "1".into(),
            remark: None,
            del_flag: "0".into(),
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
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
            .single()
            .expect("测试时间应有效");
        let persistence = Arc::new(PostFakePersistence::default());
        persistence.insert_record("tenant-a", record(timestamp));
        let service = PostService::new(persistence.clone());
        let actor = actor();

        let updated = service
            .update(
                &actor,
                7,
                UpdatePostCommand {
                    name: "新岗位".into(),
                    sort: Some(2),
                    status: "0".into(),
                },
            )
            .await
            .expect("岗位更新应成功");

        assert_eq!(updated.name, "新岗位");
        assert_eq!(updated.sort, 2);
        assert_eq!(updated.status, "0");
        let calls = persistence.calls();
        assert_eq!(calls.len(), 6);
        assert!(matches!(
            &calls[0],
            PostCall::Begin { tenant_id } if tenant_id == "tenant-a"
        ));
        assert!(matches!(
            &calls[1],
            PostCall::LockConfiguration { tenant_id } if tenant_id == "tenant-a"
        ));
        assert!(matches!(
            &calls[2],
            PostCall::FindByIdForUpdate { tenant_id, id }
                if tenant_id == "tenant-a" && *id == 7
        ));
        assert!(matches!(&calls[3], PostCall::Update { record } if record.name == "新岗位"));
        assert!(matches!(
            &calls[4],
            PostCall::IncrementConfigurationVersion { tenant_id } if tenant_id == "tenant-a"
        ));
        assert!(matches!(
            &calls[5],
            PostCall::Commit {
                audit_mode: TransactionAuditMode::CurrentRequest
            }
        ));
        assert_eq!(
            persistence.transaction_states(),
            [PostTransactionState::Committed]
        );
    }

    #[tokio::test]
    async fn update_without_sort_keeps_the_existing_value() {
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
            .single()
            .expect("测试时间应有效");
        let persistence = Arc::new(PostFakePersistence::default());
        let mut existing = record(timestamp);
        existing.sort = 17;
        persistence.insert_record("tenant-a", existing);
        let service = PostService::new(persistence.clone());

        let updated = service
            .update(
                &actor(),
                7,
                UpdatePostCommand {
                    name: "保留排序".into(),
                    sort: None,
                    status: "1".into(),
                },
            )
            .await
            .expect("未提交排序时岗位更新应成功");

        assert_eq!(updated.sort, 17);
        assert!(
            persistence
                .calls()
                .iter()
                .any(|call| matches!(call, PostCall::Update { record } if record.sort == 17))
        );
        assert_eq!(
            persistence.transaction_states(),
            [PostTransactionState::Committed]
        );
    }

    #[tokio::test]
    async fn create_rolls_back_on_duplicate_code() {
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
            .single()
            .expect("测试时间应有效");
        let persistence = Arc::new(PostFakePersistence::default());
        persistence.insert_record("tenant-a", record(timestamp));
        let service = PostService::new(persistence.clone());
        ryframe_application::install_id_generator(fixed_id).expect("测试 ID 生成器应安装成功");

        let error = service
            .create(
                &actor(),
                CreatePostCommand {
                    name: "新岗位".into(),
                    code: "old".into(),
                    sort: Some(2),
                },
            )
            .await
            .expect_err("重复岗位编码应失败");

        assert!(matches!(error, AppError::Conflict(_)));
        let calls = persistence.calls();
        assert_eq!(calls.len(), 4);
        assert!(matches!(
            &calls[0],
            PostCall::Begin { tenant_id } if tenant_id == "tenant-a"
        ));
        assert!(matches!(
            &calls[1],
            PostCall::LockConfiguration { tenant_id } if tenant_id == "tenant-a"
        ));
        assert!(matches!(
            &calls[2],
            PostCall::FindByCode {
                tenant_id,
                code,
                exclude_id: None,
            } if tenant_id == "tenant-a" && code == "old"
        ));
        assert!(matches!(&calls[3], PostCall::Rollback));
        assert_eq!(
            persistence.transaction_states(),
            [PostTransactionState::RolledBack]
        );
    }

    #[tokio::test]
    async fn exact_status_filter_treats_none_and_empty_as_unfiltered() {
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
            .single()
            .expect("测试时间应有效");
        let persistence = Arc::new(PostFakePersistence::default());
        let active = record(timestamp);
        let mut disabled = record(timestamp);
        disabled.id = 8;
        disabled.code = "disabled".into();
        disabled.status = "0".into();
        persistence.insert_record("tenant-a", active);
        persistence.insert_record("tenant-a", disabled);
        let service = PostService::new(persistence);
        let page =
            ValidatedPageQuery::new(1, 10, PaginationPolicy::new(10, 100)).expect("分页参数应有效");

        let list = |status| PostListParams {
            page,
            name: None,
            code: None,
            status,
        };
        let without_filter = service
            .find_by_page(&actor(), list(None))
            .await
            .expect("无状态过滤应成功");
        let empty_filter = service
            .find_by_page(&actor(), list(Some(String::new())))
            .await
            .expect("空状态过滤应成功");
        let active_only = service
            .find_by_page(&actor(), list(Some("1".into())))
            .await
            .expect("精确状态过滤应成功");

        assert_eq!(without_filter.total, 2);
        assert_eq!(empty_filter.total, 2);
        assert_eq!(active_only.total, 1);
        assert_eq!(active_only.records[0].status, "1");
    }

    #[tokio::test]
    async fn reads_hide_soft_deleted_rows_and_follow_declared_sort_order() {
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
            .single()
            .expect("测试时间应有效");
        let persistence = Arc::new(PostFakePersistence::default());

        let mut later = record(timestamp);
        later.sort = 20;
        let mut earlier = record(timestamp);
        earlier.id = 8;
        earlier.code = "earlier".into();
        earlier.sort = 10;
        let mut deleted = record(timestamp);
        deleted.id = 9;
        deleted.code = "deleted".into();
        deleted.sort = 0;
        deleted.del_flag = "2".into();
        persistence.insert_record("tenant-a", later);
        persistence.insert_record("tenant-a", earlier);
        persistence.insert_record("tenant-a", deleted);

        let service = PostService::new(persistence);
        let page =
            ValidatedPageQuery::new(1, 10, PaginationPolicy::new(10, 100)).expect("分页参数应有效");
        let result = service
            .find_by_page(
                &actor(),
                PostListParams {
                    page,
                    name: None,
                    code: None,
                    status: None,
                },
            )
            .await
            .expect("岗位列表读取应成功");

        assert_eq!(result.total, 2);
        assert_eq!(
            result
                .records
                .iter()
                .map(|record| record.id)
                .collect::<Vec<_>>(),
            [8, 7]
        );
        assert!(
            service
                .find_by_id(&actor(), 9)
                .await
                .expect("岗位详情读取应成功")
                .is_none()
        );
    }

    #[tokio::test]
    async fn transaction_rejects_cross_tenant_records_before_writing() {
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 20, 0, 0, 0)
            .single()
            .expect("测试时间应有效");
        let persistence = PostFakePersistence::default();
        let mut cross_tenant = record(timestamp);
        cross_tenant.id = 70;
        cross_tenant.tenant_id = "tenant-b".into();

        let insert_transaction = persistence
            .begin("tenant-a")
            .await
            .expect("岗位事务应创建成功");
        let insert_error = insert_transaction
            .insert(cross_tenant.clone())
            .await
            .expect_err("跨租户岗位不得写入");
        assert!(matches!(
            insert_error,
            AppError::Authorization(message) if message == "岗位事务租户不匹配"
        ));
        insert_transaction
            .rollback()
            .await
            .expect("拒绝写入后事务应可回滚");

        let update_transaction = persistence
            .begin("tenant-a")
            .await
            .expect("岗位事务应创建成功");
        let update_error = update_transaction
            .update(cross_tenant)
            .await
            .expect_err("跨租户岗位不得更新");
        assert!(matches!(
            update_error,
            AppError::Authorization(message) if message == "岗位事务租户不匹配"
        ));
        update_transaction
            .rollback()
            .await
            .expect("拒绝更新后事务应可回滚");

        assert!(
            !persistence
                .calls()
                .iter()
                .any(|call| { matches!(call, PostCall::Insert { .. } | PostCall::Update { .. }) })
        );
        assert_eq!(
            persistence.transaction_states(),
            [
                PostTransactionState::RolledBack,
                PostTransactionState::RolledBack
            ]
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
