use super::*;
use ryframe_application::generated::notice::NoticeListParams;

fn fixture(owner: Option<i64>) -> (Arc<NoticeFakePersistence>, NoticeService) {
    let persistence = Arc::new(NoticeFakePersistence::default());
    let mut item = record(time::macros::datetime!(2026-08-20 0:00 UTC));
    item.created_by = owner;
    persistence.insert_record("tenant-a", item);
    let service = NoticeService::new(persistence.clone());
    (persistence, service)
}

fn list() -> NoticeListParams {
    NoticeListParams {
        page: ValidatedPageQuery::new(1, 10, PaginationPolicy::new(10, 100)).unwrap(),
        title: None,
        notice_type: None,
        status: None,
    }
}

fn command() -> UpdateNoticeCommand {
    UpdateNoticeCommand {
        expected_revision: 1,
        title: "不应写入".into(),
        content_markdown: "不应写入".into(),
        notice_type: None,
        status: "0".into(),
    }
}

fn matrix_fixture() -> (Arc<NoticeFakePersistence>, NoticeService) {
    let persistence = Arc::new(NoticeFakePersistence::default());
    let timestamp = time::macros::datetime!(2026-08-20 0:00 UTC);
    for (id, owner) in [(1, Some(1)), (2, Some(2)), (3, Some(3)), (4, None)] {
        let mut item = record(timestamp);
        item.id = id;
        item.created_by = owner;
        persistence.insert_record("tenant-a", item);
    }
    persistence.set_owner_department(1, Some(10), []);
    persistence.set_owner_department(2, Some(20), [10]);
    persistence.set_owner_department(3, Some(30), []);
    let mut other_tenant = record(timestamp);
    other_tenant.tenant_id = "tenant-b".into();
    other_tenant.id = 99;
    persistence.insert_record("tenant-b", other_tenant);
    let service = NoticeService::new(persistence.clone());
    (persistence, service)
}

#[tokio::test]
async fn every_scope_has_consistent_list_detail_update_and_delete_boundaries() {
    let cases = [
        (DataScope::All, None, vec![], false, vec![1, 2, 3, 4]),
        (DataScope::SelfOnly, None, vec![], false, vec![1]),
        (DataScope::Dept, Some(10), vec![], false, vec![1]),
        (
            DataScope::DeptAndChildren,
            Some(10),
            vec![],
            false,
            vec![1, 2],
        ),
        (DataScope::Custom, None, vec![30], true, vec![1, 3]),
        (DataScope::Custom, None, vec![], false, vec![]),
        (DataScope::Dept, None, vec![], false, vec![]),
        (DataScope::DeptAndChildren, None, vec![], false, vec![]),
    ];
    for (scope, department, custom, include_self, expected) in cases {
        let mut scoped_actor = actor();
        scoped_actor.data_scope = scope.clone();
        scoped_actor.dept_id = department;
        scoped_actor.custom_dept_ids = custom;
        scoped_actor.include_self = include_self;
        let (_, service) = matrix_fixture();
        let page = service.find_by_page(&scoped_actor, list()).await.unwrap();
        let mut actual = page.records.iter().map(|item| item.id).collect::<Vec<_>>();
        actual.sort_unstable();
        assert_eq!(actual, expected, "列表范围 {scope:?}");
        assert_eq!(page.total, expected.len() as u64);
        for id in 1..=4 {
            let allowed = expected.contains(&id);
            assert_eq!(
                service
                    .find_by_id(&scoped_actor, id)
                    .await
                    .unwrap()
                    .is_some(),
                allowed,
                "详情范围 {scope:?}，记录 {id}"
            );
            verify_update_scope(&scoped_actor, id, allowed).await;
            verify_delete_scope(&scoped_actor, id, allowed).await;
        }
        assert!(
            service
                .find_by_id(&scoped_actor, 99)
                .await
                .unwrap()
                .is_none(),
            "All 也不能读取其他租户记录"
        );
    }
}

async fn verify_update_scope(actor: &ActorContext, id: i64, allowed: bool) {
    let (persistence, service) = matrix_fixture();
    let mut unrestricted = actor.clone();
    unrestricted.data_scope = DataScope::All;
    let before = service
        .find_by_id(&unrestricted, id)
        .await
        .unwrap()
        .unwrap();
    let result = service.update(actor, id, command()).await;
    let after = service
        .find_by_id(&unrestricted, id)
        .await
        .unwrap()
        .unwrap();
    if allowed {
        assert!(
            result.is_ok(),
            "允许范围内的更新：{:?}，记录 {id}",
            actor.data_scope
        );
        assert_eq!(after.title, "不应写入");
        assert_eq!(after.created_by, before.created_by);
    } else {
        assert!(matches!(result, Err(AppError::NotFound(_))));
        assert_eq!(after, before, "拒绝更新后完整记录必须不变");
        // 承接原 SelfOnly 他人记录用例，不为其他拒绝路径额外规定事务步骤。
        if matches!(actor.data_scope, DataScope::SelfOnly) && id == 2 {
            assert_eq!(
                persistence.transaction_states(),
                [NoticeTransactionState::RolledBack]
            );
        }
    }
}

async fn verify_delete_scope(actor: &ActorContext, id: i64, allowed: bool) {
    let (persistence, service) = matrix_fixture();
    let mut unrestricted = actor.clone();
    unrestricted.data_scope = DataScope::All;
    let before = service.find_by_id(&unrestricted, id).await.unwrap();
    let result = service.delete(actor, id).await;
    let after = service.find_by_id(&unrestricted, id).await.unwrap();
    if allowed {
        assert!(
            result.is_ok(),
            "允许范围内的删除：{:?}，记录 {id}",
            actor.data_scope
        );
        assert!(after.is_none());
    } else {
        assert!(matches!(result, Err(AppError::NotFound(_))));
        assert_eq!(after, before, "拒绝删除后完整记录必须不变");
        if matches!(actor.data_scope, DataScope::SelfOnly) && id == 2 {
            assert_eq!(
                persistence.transaction_states(),
                [NoticeTransactionState::RolledBack]
            );
        }
    }
}

#[tokio::test]
async fn deleted_record_is_hidden_from_scoped_and_unrestricted_operations() {
    let (_, service) = fixture(Some(1));
    let mut unrestricted = actor();
    unrestricted.data_scope = DataScope::All;
    service.delete(&unrestricted, 8).await.unwrap();
    for actor in [actor(), unrestricted] {
        assert_eq!(service.find_by_page(&actor, list()).await.unwrap().total, 0);
        assert!(service.find_by_id(&actor, 8).await.unwrap().is_none());
        assert!(matches!(
            service.update(&actor, 8, command()).await,
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            service.delete(&actor, 8).await,
            Err(AppError::NotFound(_))
        ));
    }
}
