//! UserRepository tenant-boundary tests.

mod common;

use chrono::Utc;
use common::setup_test_db;
use ryframe_common::utils::snowflake;
use ryframe_core::repository::Repository;
use ryframe_db::{UserRepository, entities::user};

fn make_user(tenant_id: &str, username: &str) -> user::Model {
    user::Model {
        id: snowflake::next_snowflake_id(),
        tenant_id: tenant_id.into(),
        username: username.into(),
        password_hash: "test-hash".into(),
        nickname: username.into(),
        email: format!("{username}@test.local"),
        phone: String::new(),
        avatar: None,
        status: user::Model::STATUS_NORMAL.into(),
        auth_version: 0,
        dept_id: None,
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[tokio::test]
async fn generic_queries_and_writes_cannot_cross_tenants() {
    let db = setup_test_db().await;
    let repo = UserRepository;
    let system_user = repo
        .insert(&db, "system", make_user("system", "system-user"))
        .await
        .expect("insert system user");

    let other_user = repo
        .insert(&db, "other", make_user("other", "other-user"))
        .await
        .expect("insert other tenant user");

    assert!(
        repo.find_by_id(&db, "system", other_user.id)
            .await
            .expect("find other id")
            .is_none()
    );
    assert!(
        repo.find_by_username(&db, "system", "other-user")
            .await
            .expect("find other name")
            .is_none()
    );
    assert_eq!(
        repo.find_by_id(&db, "other", other_user.id)
            .await
            .expect("explicit tenant id")
            .expect("other user exists")
            .id,
        other_user.id
    );
    assert_eq!(
        repo.find_by_username(&db, "other", "other-user")
            .await
            .expect("explicit tenant name")
            .expect("other user exists")
            .id,
        other_user.id
    );

    let mut updated = system_user.clone();
    updated.nickname = "updated".into();
    repo.update(&db, "system", updated)
        .await
        .expect("update current tenant user");
    assert_eq!(
        repo.find_by_id(&db, "system", system_user.id)
            .await
            .expect("find updated user")
            .expect("system user exists")
            .nickname,
        "updated"
    );

    assert!(
        repo.insert(&db, "system", make_user("other", "invalid-write"))
            .await
            .is_err()
    );
}
