mod common;

fn page_query(page: u64, page_size: u64) -> ryframe_core::ValidatedPageQuery {
    ryframe_core::ValidatedPageQuery::new(
        page,
        page_size,
        &ryframe_config::PaginationConfig::default(),
    )
    .expect("测试分页参数必须有效")
}

use chrono::Utc;
use ryframe_db::{
    UserRepository,
    entities::{dept, user},
};
use ryframe_kernel::{DataScope, DataScopeContext};
use sea_orm::{ActiveModelTrait, ActiveValue};

async fn insert_dept(
    db: &sea_orm::DatabaseConnection,
    id: i64,
    parent_id: Option<i64>,
    ancestors: &str,
) {
    dept::ActiveModel {
        id: ActiveValue::Set(id),
        tenant_id: ActiveValue::Set("system".into()),
        name: ActiveValue::Set(format!("部门{id}")),
        parent_id: ActiveValue::Set(parent_id),
        ancestors: ActiveValue::Set(ancestors.into()),
        sort: ActiveValue::Set(id as i32),
        status: ActiveValue::Set("1".into()),
        remark: ActiveValue::Set(None),
        del_flag: ActiveValue::Set("0".into()),
        created_at: ActiveValue::Set(Utc::now()),
        updated_at: ActiveValue::Set(Utc::now()),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn insert_user(
    db: &sea_orm::DatabaseConnection,
    id: i64,
    dept_id: Option<i64>,
    tenant_id: &str,
) {
    user::ActiveModel {
        id: ActiveValue::Set(id),
        tenant_id: ActiveValue::Set(tenant_id.into()),
        username: ActiveValue::Set(format!("user{id}")),
        password_hash: ActiveValue::Set("hash".into()),
        nickname: ActiveValue::Set(format!("用户{id}")),
        email: ActiveValue::Set(String::new()),
        phone: ActiveValue::Set(String::new()),
        avatar: ActiveValue::Set(None),
        avatar_file_id: ActiveValue::Set(None),
        preferred_locale: ActiveValue::Set(None),
        status: ActiveValue::Set("1".into()),
        authorization_version: ActiveValue::Set(1),
        dept_id: ActiveValue::Set(dept_id),
        remark: ActiveValue::Set(None),
        login_ip: ActiveValue::Set(None),
        login_date: ActiveValue::Set(None),
        del_flag: ActiveValue::Set("0".into()),
        created_at: ActiveValue::Set(Utc::now()),
        updated_at: ActiveValue::Set(Utc::now()),
    }
    .insert(db)
    .await
    .unwrap();
}

fn context(
    scope: DataScope,
    user_id: i64,
    dept_id: Option<i64>,
    ids: Vec<i64>,
    include_self: bool,
) -> DataScopeContext {
    DataScopeContext {
        scope,
        user_id,
        dept_id,
        ancestors: None,
        custom_dept_ids: ids,
        include_self,
    }
}

async fn visible_ids(
    db: &sea_orm::DatabaseConnection,
    tenant_id: &str,
    ctx: &DataScopeContext,
) -> Vec<i64> {
    let mut ids = UserRepository
        .find_by_page_with_data_scope(db, tenant_id, page_query(1, 100), ctx)
        .await
        .unwrap()
        .records
        .into_iter()
        .map(|item| item.id)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

async fn visible_option_ids(
    db: &sea_orm::DatabaseConnection,
    tenant_id: &str,
    ctx: &DataScopeContext,
) -> Vec<i64> {
    let mut ids = UserRepository
        .find_options_with_data_scope(db, tenant_id, None, ctx, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|item| item.id)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

#[tokio::test]
async fn user_repository_enforces_every_data_scope_and_mixed_role_union() {
    let db = common::setup_test_db().await;
    insert_dept(&db, 1, None, "0").await;
    insert_dept(&db, 2, Some(1), "0,1").await;
    insert_dept(&db, 3, None, "0").await;
    insert_user(&db, 10, Some(1), "system").await;
    insert_user(&db, 11, Some(2), "system").await;
    insert_user(&db, 12, Some(3), "system").await;
    insert_user(&db, 13, None, "system").await;
    insert_user(&db, 14, None, "tenant-b").await;

    assert_eq!(
        visible_ids(
            &db,
            "system",
            &context(DataScope::Dept, 10, Some(1), vec![], false)
        )
        .await,
        vec![10]
    );
    assert_eq!(
        visible_ids(
            &db,
            "system",
            &context(DataScope::DeptAndChildren, 10, Some(1), vec![], false)
        )
        .await,
        vec![10, 11]
    );
    assert_eq!(
        visible_ids(
            &db,
            "system",
            &context(DataScope::Custom, 10, Some(1), vec![3], false)
        )
        .await,
        vec![12]
    );
    assert_eq!(
        visible_ids(
            &db,
            "system",
            &context(DataScope::SelfOnly, 11, Some(2), vec![], true)
        )
        .await,
        vec![11]
    );
    assert_eq!(
        visible_ids(
            &db,
            "system",
            &context(DataScope::Custom, 10, Some(1), vec![3], true)
        )
        .await,
        vec![10, 12]
    );
    let tenant_b_ids = visible_ids(&db, "tenant-b", &DataScopeContext::super_admin(14)).await;
    assert_eq!(tenant_b_ids, vec![14]);

    // 候选接口复用分页用户列表的数据范围与租户边界，不能扩大可见集合。
    let option_scope = context(DataScope::DeptAndChildren, 10, Some(1), vec![], false);
    assert_eq!(
        visible_option_ids(&db, "system", &option_scope).await,
        visible_ids(&db, "system", &option_scope).await
    );
    assert_eq!(
        visible_option_ids(&db, "tenant-b", &DataScopeContext::super_admin(14)).await,
        vec![14]
    );

    let prefixed = UserRepository
        .find_options_with_data_scope(
            &db,
            "system",
            Some("user1"),
            &DataScopeContext::super_admin(10),
            2,
        )
        .await
        .unwrap();
    assert_eq!(
        prefixed
            .into_iter()
            .map(|item| item.username)
            .collect::<Vec<_>>(),
        vec!["user10", "user11"]
    );
}
