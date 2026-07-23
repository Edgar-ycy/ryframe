//! RoleRepository integration tests.

mod common;

use chrono::Utc;
use common::setup_test_db;
use ryframe_common::utils::snowflake;
use ryframe_core::repository::{PageQuery, Repository};
use ryframe_db::{
    DeptRepository, RoleRepository,
    entities::{dept, role, user},
};
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, TransactionTrait};

const TENANT: &str = "system";

fn make_role(name: &str, code: &str, status: &str) -> role::Model {
    role::Model {
        id: snowflake::try_next_snowflake_id().expect("generate test ID"),
        tenant_id: TENANT.into(),
        name: name.into(),
        code: code.into(),
        is_super: 0,
        data_scope: role::Model::DATA_SCOPE_ALL.into(),
        status: status.into(),
        sort: 1,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

async fn insert_role(db: &DatabaseConnection, name: &str, code: &str) -> role::Model {
    RoleRepository
        .insert(
            db,
            TENANT,
            make_role(name, code, role::Model::STATUS_NORMAL),
        )
        .await
        .expect("insert role")
}

async fn insert_user(db: &DatabaseConnection, username: &str) -> i64 {
    let id = snowflake::try_next_snowflake_id().expect("generate test ID");
    user::ActiveModel {
        id: sea_orm::ActiveValue::Set(id),
        tenant_id: sea_orm::ActiveValue::Set(TENANT.into()),
        username: sea_orm::ActiveValue::Set(username.into()),
        password_hash: sea_orm::ActiveValue::Set("test-hash".into()),
        nickname: sea_orm::ActiveValue::Set(username.into()),
        email: sea_orm::ActiveValue::Set(format!("{username}@test.local")),
        phone: sea_orm::ActiveValue::Set(String::new()),
        avatar: sea_orm::ActiveValue::Set(None),
        status: sea_orm::ActiveValue::Set(user::Model::STATUS_NORMAL.into()),
        auth_version: sea_orm::ActiveValue::Set(0),
        dept_id: sea_orm::ActiveValue::Set(None),
        remark: sea_orm::ActiveValue::Set(None),
        login_ip: sea_orm::ActiveValue::Set(None),
        login_date: sea_orm::ActiveValue::Set(None),
        del_flag: sea_orm::ActiveValue::Set(user::Model::DEL_FLAG_NORMAL.into()),
        created_at: sea_orm::ActiveValue::Set(Utc::now()),
        updated_at: sea_orm::ActiveValue::Set(Utc::now()),
    }
    .insert(db)
    .await
    .expect("insert user");
    id
}

async fn insert_dept(db: &DatabaseConnection, name: &str) -> i64 {
    let model = dept::Model {
        id: snowflake::try_next_snowflake_id().expect("generate test ID"),
        tenant_id: TENANT.into(),
        name: name.into(),
        parent_id: None,
        ancestors: "0".into(),
        sort: 1,
        status: dept::Model::STATUS_NORMAL.into(),
        remark: None,
        del_flag: dept::Model::DEL_FLAG_NORMAL.into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    DeptRepository
        .insert(db, TENANT, model)
        .await
        .expect("insert department")
        .id
}

#[tokio::test]
async fn crud_filter_and_batch_delete_are_consistent() {
    let db = setup_test_db().await;
    let repo = RoleRepository;
    let admin = insert_role(&db, "管理员", "admin").await;
    let auditor = insert_role(&db, "审计员", "auditor").await;

    let page = repo
        .find_by_page(
            &db,
            TENANT,
            PageQuery {
                page: 1,
                page_size: 1,
            },
        )
        .await
        .expect("page roles");
    assert_eq!(page.total, 2);
    assert_eq!(page.records.len(), 1);

    let filtered = repo
        .find_by_page_filtered(
            &db,
            TENANT,
            PageQuery::default(),
            Some("管理"),
            Some("admin"),
            Some(role::Model::STATUS_NORMAL),
        )
        .await
        .expect("filter roles");
    assert_eq!(filtered.records.len(), 1);

    assert_eq!(
        repo.find_by_code(&db, TENANT, "admin")
            .await
            .expect("find by code")
            .expect("admin exists")
            .id,
        admin.id
    );
    assert_eq!(
        repo.find_by_ids(&db, TENANT, &[admin.id, auditor.id])
            .await
            .expect("find by ids")
            .len(),
        2
    );
    assert!(
        repo.find_by_ids(&db, TENANT, &[])
            .await
            .expect("empty ids")
            .is_empty()
    );

    let mut updated = admin.clone();
    updated.name = "系统管理员".into();
    repo.update(&db, TENANT, updated)
        .await
        .expect("update role");
    assert_eq!(
        repo.find_by_id(&db, TENANT, admin.id)
            .await
            .expect("find role")
            .expect("role exists")
            .name,
        "系统管理员"
    );

    assert_eq!(
        repo.delete_many(&db, TENANT, &[admin.id])
            .await
            .expect("delete role"),
        1
    );
    assert!(
        repo.find_by_id(&db, TENANT, admin.id)
            .await
            .expect("find deleted")
            .is_none()
    );
    assert_eq!(
        repo.delete_many(&db, TENANT, &[])
            .await
            .expect("delete empty"),
        0
    );
}

#[tokio::test]
async fn role_replacements_support_clear_commit_and_rollback() {
    let db = setup_test_db().await;
    let repo = RoleRepository;
    let user_id = insert_user(&db, "role-user").await;
    let active = insert_role(&db, "可用角色", "active-role").await;
    let mut disabled_model = make_role("停用角色", "disabled-role", role::Model::STATUS_DISABLED);
    disabled_model.sort = 2;
    let disabled = repo
        .insert(&db, TENANT, disabled_model)
        .await
        .expect("insert disabled role");

    repo.replace_roles(&db, TENANT, user_id, &[active.id, disabled.id])
        .await
        .expect("assign roles");
    assert_eq!(
        repo.find_user_roles(&db, TENANT, user_id)
            .await
            .expect("active roles")
            .len(),
        1
    );
    assert_eq!(
        repo.find_user_roles_all_status(&db, TENANT, user_id)
            .await
            .expect("all roles")
            .len(),
        2
    );
    assert_eq!(
        repo.find_user_ids_by_role_ids(&db, TENANT, &[active.id, disabled.id])
            .await
            .expect("role users"),
        vec![user_id]
    );
    assert!(
        repo.find_user_ids_by_role_ids(&db, TENANT, &[])
            .await
            .expect("empty roles")
            .is_empty()
    );

    repo.replace_roles(&db, TENANT, user_id, &[])
        .await
        .expect("clear with replace");
    assert!(
        repo.find_user_roles_all_status(&db, TENANT, user_id)
            .await
            .expect("cleared roles")
            .is_empty()
    );

    let transaction = db.begin().await.expect("begin transaction");
    repo.replace_roles_in_txn(&transaction, TENANT, user_id, &[active.id])
        .await
        .expect("assign in transaction");
    transaction.commit().await.expect("commit transaction");
    assert_eq!(
        repo.find_user_roles(&db, TENANT, user_id)
            .await
            .expect("assigned roles")
            .len(),
        1
    );

    let transaction = db.begin().await.expect("begin rollback transaction");
    repo.replace_roles_in_txn(&transaction, TENANT, user_id, &[])
        .await
        .expect("clear roles in transaction");
    transaction.rollback().await.expect("rollback transaction");
    assert_eq!(
        repo.find_user_roles(&db, TENANT, user_id)
            .await
            .expect("roles after rollback")
            .len(),
        1
    );

    repo.replace_roles(&db, TENANT, user_id, &[])
        .await
        .expect("atomically clear roles");
    assert!(
        repo.find_user_roles(&db, TENANT, user_id)
            .await
            .expect("cleared roles")
            .is_empty()
    );
}

#[tokio::test]
async fn data_scope_and_super_role_queries_are_covered() {
    let db = setup_test_db().await;
    let repo = RoleRepository;
    let first = insert_role(&db, "范围角色一", "scope-one").await;
    let second = insert_role(&db, "范围角色二", "scope-two").await;
    let dept_a = insert_dept(&db, "研发部").await;
    let dept_b = insert_dept(&db, "审计部").await;

    repo.replace_data_scope(
        &db,
        TENANT,
        first.id,
        role::Model::DATA_SCOPE_CUSTOM,
        &[dept_a, dept_b],
    )
    .await
    .expect("assign first scope");
    repo.replace_data_scope(
        &db,
        TENANT,
        second.id,
        role::Model::DATA_SCOPE_CUSTOM,
        &[dept_b],
    )
    .await
    .expect("assign second scope");
    assert_eq!(
        repo.find_role_dept_ids(&db, TENANT, first.id)
            .await
            .expect("first scope")
            .len(),
        2
    );
    assert_eq!(
        repo.find_roles_dept_ids(&db, TENANT, &[first.id, second.id])
            .await
            .expect("merged scope"),
        vec![dept_a.min(dept_b), dept_a.max(dept_b)]
    );

    assert_eq!(
        repo.find_by_id(&db, TENANT, first.id)
            .await
            .expect("find role")
            .expect("role exists")
            .data_scope,
        role::Model::DATA_SCOPE_CUSTOM
    );

    let mut super_role = make_role("超级角色", "super-role", role::Model::STATUS_NORMAL);
    super_role.is_super = 1;
    let super_role = repo
        .insert(&db, TENANT, super_role)
        .await
        .expect("insert super role");
    assert_eq!(
        repo.find_super_role(&db, TENANT)
            .await
            .expect("find super role")
            .expect("super role exists")
            .id,
        super_role.id
    );
}

#[tokio::test]
async fn replacing_data_scope_rolls_back_role_and_departments_together() {
    let db = setup_test_db().await;
    let repo = RoleRepository;
    let target = insert_role(&db, "事务范围角色", "scope-rollback").await;
    let dept_id = insert_dept(&db, "事务部门").await;
    db.execute_unprepared(
        "CREATE TRIGGER reject_role_dept BEFORE INSERT ON sys_role_dept \
         FOR EACH ROW SIGNAL SQLSTATE '45000' \
         SET MESSAGE_TEXT = 'forced role dept failure'",
    )
    .await
    .expect("create rejection trigger");

    let result = repo
        .replace_data_scope(
            &db,
            TENANT,
            target.id,
            role::Model::DATA_SCOPE_CUSTOM,
            &[dept_id],
        )
        .await;

    assert!(result.is_err());
    assert_eq!(
        repo.find_by_id(&db, TENANT, target.id)
            .await
            .expect("find role")
            .expect("role exists")
            .data_scope,
        role::Model::DATA_SCOPE_ALL
    );
    assert!(
        repo.find_role_dept_ids(&db, TENANT, target.id)
            .await
            .expect("find departments")
            .is_empty()
    );
}

#[tokio::test]
async fn pagination_is_tenant_scoped() {
    let db = setup_test_db().await;
    insert_role(&db, "系统角色", "system-only").await;

    let mut other = make_role("其他租户角色", "other-only", role::Model::STATUS_NORMAL);
    other.tenant_id = "other".into();
    RoleRepository
        .insert(&db, "other", other)
        .await
        .expect("insert other tenant role");

    let system_page = RoleRepository
        .find_by_page(&db, TENANT, PageQuery::default())
        .await
        .expect("system page");
    assert_eq!(system_page.total, 1);

    let other_page = RoleRepository
        .find_by_page(&db, "other", PageQuery::default())
        .await
        .expect("other page");
    assert_eq!(other_page.total, 1);
}
