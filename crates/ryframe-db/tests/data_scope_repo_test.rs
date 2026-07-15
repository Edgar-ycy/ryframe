mod common;

use chrono::Utc;
use ryframe_common::{DataScope, DataScopeContext};
use ryframe_core::PageQuery;
use ryframe_db::{
    UserRepository,
    entities::{dept, user},
};
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
        status: ActiveValue::Set("1".into()),
        auth_version: ActiveValue::Set(1),
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

async fn visible_ids(db: &sea_orm::DatabaseConnection, ctx: &DataScopeContext) -> Vec<i64> {
    let mut ids = UserRepository
        .find_by_page_with_data_scope(db, PageQuery::all_records(), ctx)
        .await
        .unwrap()
        .records
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
        visible_ids(&db, &context(DataScope::Dept, 10, Some(1), vec![], false)).await,
        vec![10]
    );
    assert_eq!(
        visible_ids(
            &db,
            &context(DataScope::DeptAndChildren, 10, Some(1), vec![], false)
        )
        .await,
        vec![10, 11]
    );
    assert_eq!(
        visible_ids(
            &db,
            &context(DataScope::Custom, 10, Some(1), vec![3], false)
        )
        .await,
        vec![12]
    );
    assert_eq!(
        visible_ids(
            &db,
            &context(DataScope::SelfOnly, 11, Some(2), vec![], true)
        )
        .await,
        vec![11]
    );
    assert_eq!(
        visible_ids(&db, &context(DataScope::Custom, 10, Some(1), vec![3], true)).await,
        vec![10, 12]
    );
    let tenant_b_ids = ryframe_core::multi_tenant::with_tenant_context(
        ryframe_core::multi_tenant::TenantContext {
            tenant_id: "tenant-b".into(),
            is_admin: false,
        },
        visible_ids(&db, &DataScopeContext::super_admin(14)),
    )
    .await;
    assert_eq!(tenant_b_ids, vec![14]);
}
