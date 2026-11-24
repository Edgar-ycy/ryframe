use ryframe_kernel::DataScope;
use time::{OffsetDateTime, macros::datetime};

#[path = "notice_data_scope.rs"]
mod data_scope;

use super::*;
use ryframe_application::{
    TransactionAuditMode,
    generated::notice::{
        NoticeCall, NoticeFailure, NoticeFakePersistence, NoticeRecord, NoticeService,
        NoticeTransactionState, UpdateNoticeCommand,
    },
};

fn record(timestamp: OffsetDateTime) -> NoticeRecord {
    NoticeRecord {
        id: 8,
        tenant_id: "tenant-a".into(),
        title: "旧通知".into(),
        content_markdown: "旧内容".into(),
        notice_type: Some("1".into()),
        status: "1".into(),
        created_by: Some(1),
        del_flag: "0".into(),
        created_at: timestamp,
        updated_at: timestamp,
        revision: 1,
    }
}

#[tokio::test]
async fn update_failure_rolls_back_the_same_transaction() {
    let timestamp = datetime!(2026-08-20 0:00 UTC);
    let persistence = Arc::new(NoticeFakePersistence::default());
    persistence.insert_record("tenant-a", record(timestamp));
    persistence.fail_next(NoticeFailure::Update);
    let service = NoticeService::new(persistence.clone());

    let error = service
        .update(
            &actor(),
            8,
            UpdateNoticeCommand {
                expected_revision: 1,
                title: "失败通知".into(),
                content_markdown: "失败内容".into(),
                notice_type: None,
                status: "0".into(),
            },
        )
        .await
        .expect_err("持久化失败应返回错误");

    assert!(matches!(error, AppError::Database(_)));
    assert!(matches!(
        persistence.calls().as_slice(),
        [
            NoticeCall::Begin { .. },
            NoticeCall::FindByIdForUpdate { .. },
            NoticeCall::Update { .. },
            NoticeCall::Rollback,
        ]
    ));
    assert_eq!(
        persistence.transaction_states(),
        [NoticeTransactionState::RolledBack]
    );
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
        data_scope_version: 1,
        is_super_admin: false,
    }
}

#[tokio::test]
async fn update_locks_row_without_configuration_version_transaction_steps() {
    let timestamp = datetime!(2026-08-20 0:00 UTC);
    let persistence = Arc::new(NoticeFakePersistence::default());
    persistence.insert_record("tenant-a", record(timestamp));
    let service = NoticeService::new(persistence.clone());

    let updated = service
        .update(
            &actor(),
            8,
            UpdateNoticeCommand {
                expected_revision: 1,
                title: "新通知".into(),
                content_markdown: "新内容".into(),
                notice_type: Some("2".into()),
                status: "0".into(),
            },
        )
        .await
        .expect("通知更新应成功");

    assert_eq!(updated.title, "新通知");
    assert_eq!(updated.content_markdown, "新内容");
    assert_eq!(updated.notice_type.as_deref(), Some("2"));
    assert_eq!(updated.revision, 2);
    assert_eq!(
        persistence.calls(),
        [
            NoticeCall::Begin {
                tenant_id: "tenant-a".into(),
            },
            NoticeCall::FindByIdForUpdate {
                tenant_id: "tenant-a".into(),
                id: 8,
            },
            NoticeCall::Update {
                record: updated.clone(),
            },
            NoticeCall::Commit {
                audit_mode: TransactionAuditMode::CurrentRequest,
            },
        ]
    );
    assert_eq!(
        persistence.transaction_states(),
        [NoticeTransactionState::Committed]
    );

    let stale = service
        .update(
            &actor(),
            8,
            UpdateNoticeCommand {
                expected_revision: 1,
                title: "过期通知".into(),
                content_markdown: "过期内容".into(),
                notice_type: None,
                status: "1".into(),
            },
        )
        .await;
    assert!(matches!(stale, Err(AppError::Conflict(_))));
    assert!(matches!(
        &persistence.calls()[4..],
        [
            NoticeCall::Begin { .. },
            NoticeCall::FindByIdForUpdate { .. },
            NoticeCall::Rollback,
        ]
    ));
    assert_eq!(
        persistence.transaction_states(),
        [
            NoticeTransactionState::Committed,
            NoticeTransactionState::RolledBack,
        ]
    );
    assert_eq!(
        service.find_by_id(&actor(), 8).await.unwrap(),
        Some(updated)
    );
}
