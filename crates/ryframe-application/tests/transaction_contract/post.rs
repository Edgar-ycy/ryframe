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
    crate::ensure_test_id_generator();

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
