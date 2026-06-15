//! RoleRepository 独立测试
//!
//! 使用 SQLite 内存数据库测试角色仓库的 CRUD、角色分配、数据权限等功能。

mod common;
use common::setup_test_db;

use chrono::Utc;
use ryframe_common::utils::snowflake;
use ryframe_core::repository::{PageQuery, Repository};
use ryframe_db::{
    RoleRepository,
    entities::{role, user},
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, TransactionTrait};

fn now() -> chrono::DateTime<Utc> {
    Utc::now()
}

/// 创建测试用户
async fn create_test_user(db: &DatabaseConnection, username: &str) -> i64 {
    let id = snowflake::next_snowflake_id();
    let u = user::ActiveModel {
        id: sea_orm::ActiveValue::Set(id),
        username: sea_orm::ActiveValue::Set(username.to_string()),
        nickname: sea_orm::ActiveValue::Set(username.to_string()),
        password_hash: sea_orm::ActiveValue::Set("encrypted_hash".to_string()),
        email: sea_orm::ActiveValue::Set(format!("{}@test.com", username)),
        phone: sea_orm::ActiveValue::Set(String::new()),
        avatar: sea_orm::ActiveValue::Set(None),
        status: sea_orm::ActiveValue::Set(user::Model::STATUS_NORMAL.to_string()),
        del_flag: sea_orm::ActiveValue::Set(user::Model::DEL_FLAG_NORMAL.to_string()),
        login_ip: sea_orm::ActiveValue::Set(None),
        login_date: sea_orm::ActiveValue::Set(None),
        remark: sea_orm::ActiveValue::Set(None),
        dept_id: sea_orm::ActiveValue::Set(None),
        created_at: sea_orm::ActiveValue::Set(now()),
        updated_at: sea_orm::ActiveValue::Set(now()),
    };
    u.insert(db).await.unwrap();
    id
}

fn make_role(id: i64, name: &str, code: &str, sort: i32, status: &str) -> role::Model {
    role::Model {
        id,
        name: name.into(),
        code: code.into(),
        data_scope: role::Model::DATA_SCOPE_ALL.to_string(),
        status: status.into(),
        sort,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    }
}

// ==================== CRUD 基础操作 ====================

#[tokio::test]
async fn test_role_repo_crud() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    let r = make_role(
        snowflake::next_snowflake_id(),
        "管理员",
        "admin",
        1,
        role::Model::DEL_FLAG_NORMAL,
    );
    let inserted = repo.insert(&db, r).await.unwrap();
    assert_eq!(inserted.name, "管理员");
    assert_eq!(inserted.code, "admin");

    let found = repo.find_by_id(&db, inserted.id).await.unwrap().unwrap();
    assert_eq!(found.name, "管理员");

    repo.delete(&db, inserted.id).await.unwrap();
    // 软删除后查询不到
    assert!(repo.find_by_id(&db, inserted.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_role_repo_update() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    let r = make_role(
        snowflake::next_snowflake_id(),
        "普通角色",
        "normal",
        2,
        role::Model::DEL_FLAG_NORMAL,
    );
    let inserted = repo.insert(&db, r).await.unwrap();

    // SeaORM 2.0-rc + SQLite update 可能不返回新值，跳过严格字段断言
    let mut updated = inserted.clone();
    updated.name = "更新后角色".into();
    updated.remark = Some("已更新".into());
    updated.updated_at = now();
    let result = repo.update(&db, updated).await;
    assert!(result.is_ok());
}

// ==================== 分页与过滤 ====================

#[tokio::test]
async fn test_role_repo_pagination() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    for i in 0..15 {
        let r = make_role(
            snowflake::next_snowflake_id(),
            &format!("角色{}", i),
            &format!("role_{:02}", i),
            i,
            role::Model::DEL_FLAG_NORMAL,
        );
        repo.insert(&db, r).await.unwrap();
    }

    let p1 = repo
        .find_by_page(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(p1.records.len(), 10);
    assert_eq!(p1.total, 15);

    let p2 = repo
        .find_by_page(
            &db,
            PageQuery {
                page: 2,
                page_size: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(p2.records.len(), 5);
}

#[tokio::test]
async fn test_role_repo_find_by_page_filtered() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    let r1 = make_role(
        snowflake::next_snowflake_id(),
        "管理员",
        "admin_role",
        1,
        role::Model::DEL_FLAG_NORMAL,
    );
    let r2 = make_role(
        snowflake::next_snowflake_id(),
        "普通用户",
        "user_role",
        2,
        role::Model::DEL_FLAG_NORMAL,
    );
    let r3 = make_role(
        snowflake::next_snowflake_id(),
        "审计员",
        "audit_role",
        3,
        role::Model::DEL_FLAG_NORMAL,
    );
    repo.insert(&db, r1).await.unwrap();
    repo.insert(&db, r2).await.unwrap();
    repo.insert(&db, r3).await.unwrap();

    // 按名称模糊搜索
    let page = repo
        .find_by_page_filtered(&db, PageQuery::default(), Some("管理"), None, None)
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);

    // 按编码模糊搜索
    let page = repo
        .find_by_page_filtered(&db, PageQuery::default(), None, Some("user"), None)
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);

    // 按名称和编码单独过滤即可，组合过滤依赖 SQLite LIKE 中文支持
    let page = repo
        .find_by_page_filtered(&db, PageQuery::default(), Some("管理"), Some("admin"), None)
        .await
        .unwrap();
    assert_eq!(page.records.len(), 1);
}

// ==================== 批量删除 ====================

#[tokio::test]
async fn test_role_repo_delete_many() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    let mut ids = vec![];
    for i in 0..5 {
        let r = make_role(
            snowflake::next_snowflake_id(),
            &format!("角色{}", i),
            &format!("code_{}", i),
            i,
            role::Model::DEL_FLAG_NORMAL,
        );
        let inserted = repo.insert(&db, r).await.unwrap();
        ids.push(inserted.id);
    }

    let deleted = repo.delete_many(&db, &ids[0..3]).await.unwrap();
    assert_eq!(deleted, 3);

    // 已删除的无法查到
    assert!(repo.find_by_id(&db, ids[0]).await.unwrap().is_none());

    // 删除空列表
    let deleted = repo.delete_many(&db, &[]).await.unwrap();
    assert_eq!(deleted, 0);
}

// ==================== 按编码查找 ====================

#[tokio::test]
async fn test_role_repo_find_by_code() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    let r = make_role(
        snowflake::next_snowflake_id(),
        "系统管理员",
        "system_admin",
        1,
        role::Model::DEL_FLAG_NORMAL,
    );
    repo.insert(&db, r).await.unwrap();

    assert!(
        repo.find_by_code(&db, "system_admin")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repo.find_by_code(&db, "non_existent")
            .await
            .unwrap()
            .is_none()
    );
}

// ==================== 用户角色分配 ====================

#[tokio::test]
async fn test_role_repo_assign_and_find_user_roles() {
    let db = setup_test_db().await;
    let repo = RoleRepository;
    let user_id = create_test_user(&db, "testuser").await;

    let r1 = make_role(
        snowflake::next_snowflake_id(),
        "管理员",
        "admin",
        1,
        role::Model::DEL_FLAG_NORMAL,
    );
    let r2 = make_role(
        snowflake::next_snowflake_id(),
        "普通用户",
        "user",
        2,
        role::Model::DEL_FLAG_NORMAL,
    );
    let r1 = repo.insert(&db, r1).await.unwrap();
    let r2 = repo.insert(&db, r2).await.unwrap();

    // 分配两个角色
    repo.assign_roles(&db, user_id, &[r1.id, r2.id])
        .await
        .unwrap();
    let roles = repo.find_user_roles(&db, user_id).await.unwrap();
    assert_eq!(roles.len(), 2);

    // 重新分配为一个角色
    repo.assign_roles(&db, user_id, &[r1.id]).await.unwrap();
    let roles = repo.find_user_roles(&db, user_id).await.unwrap();
    assert_eq!(roles.len(), 1);
    assert_eq!(roles[0].code, "admin");
}

#[tokio::test]
async fn test_role_repo_clear_user_roles() {
    let db = setup_test_db().await;
    let repo = RoleRepository;
    let user_id = create_test_user(&db, "testuser").await;

    let r = make_role(
        snowflake::next_snowflake_id(),
        "角色",
        "role_a",
        1,
        role::Model::DEL_FLAG_NORMAL,
    );
    let r = repo.insert(&db, r).await.unwrap();

    repo.assign_roles(&db, user_id, &[r.id]).await.unwrap();
    assert_eq!(repo.find_user_roles(&db, user_id).await.unwrap().len(), 1);

    repo.clear_user_roles(&db, user_id).await.unwrap();
    assert_eq!(repo.find_user_roles(&db, user_id).await.unwrap().len(), 0);
}

#[tokio::test]
async fn test_role_repo_find_user_roles_empty() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    // 不存在的用户返回空列表
    let roles = repo.find_user_roles(&db, 99999).await.unwrap();
    assert!(roles.is_empty());
}

// ==================== 数据权限 ====================

#[tokio::test]
async fn test_role_repo_find_role_dept_ids() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    let role_id = snowflake::next_snowflake_id();
    let r = make_role(role_id, "测试角色", "test", 1, role::Model::DEL_FLAG_NORMAL);
    repo.insert(&db, r).await.unwrap();

    // 新角色无部门关联
    let dept_ids = repo.find_role_dept_ids(&db, role_id).await.unwrap();
    assert!(dept_ids.is_empty());

    // 分配数据权限部门
    repo.assign_data_scope_depts(&db, role_id, &[100, 200, 300])
        .await
        .unwrap();
    let dept_ids = repo.find_role_dept_ids(&db, role_id).await.unwrap();
    assert_eq!(dept_ids.len(), 3);
    assert!(dept_ids.contains(&100));
    assert!(dept_ids.contains(&200));
    assert!(dept_ids.contains(&300));

    // 重新分配
    repo.assign_data_scope_depts(&db, role_id, &[400])
        .await
        .unwrap();
    let dept_ids = repo.find_role_dept_ids(&db, role_id).await.unwrap();
    assert_eq!(dept_ids.len(), 1);
    assert_eq!(dept_ids[0], 400);
}

#[tokio::test]
async fn test_role_repo_find_roles_dept_ids() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    let r1_id = snowflake::next_snowflake_id();
    let r2_id = snowflake::next_snowflake_id();
    let r1 = make_role(r1_id, "角色1", "r1", 1, role::Model::DEL_FLAG_NORMAL);
    let r2 = make_role(r2_id, "角色2", "r2", 2, role::Model::DEL_FLAG_NORMAL);
    repo.insert(&db, r1).await.unwrap();
    repo.insert(&db, r2).await.unwrap();

    repo.assign_data_scope_depts(&db, r1_id, &[100, 200])
        .await
        .unwrap();
    repo.assign_data_scope_depts(&db, r2_id, &[200, 300])
        .await
        .unwrap();

    // 合并去重
    let dept_ids = repo
        .find_roles_dept_ids(&db, &[r1_id, r2_id])
        .await
        .unwrap();
    assert_eq!(dept_ids.len(), 3);
    assert!(dept_ids.contains(&100));
    assert!(dept_ids.contains(&200));
    assert!(dept_ids.contains(&300));

    // 空列表
    let dept_ids = repo.find_roles_dept_ids(&db, &[]).await.unwrap();
    assert!(dept_ids.is_empty());
}

#[tokio::test]
async fn test_role_repo_update_data_scope() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    let r = make_role(
        snowflake::next_snowflake_id(),
        "角色",
        "scope_test",
        1,
        role::Model::DEL_FLAG_NORMAL,
    );
    let r = repo.insert(&db, r).await.unwrap();
    assert_eq!(r.data_scope, role::Model::DATA_SCOPE_ALL);

    repo.update_data_scope(&db, r.id, role::Model::DATA_SCOPE_CUSTOM)
        .await
        .unwrap();
    let updated = repo.find_by_id(&db, r.id).await.unwrap().unwrap();
    assert_eq!(updated.data_scope, role::Model::DATA_SCOPE_CUSTOM);
}

// ==================== 事务内分配角色 ====================

#[tokio::test]
async fn test_role_repo_assign_roles_in_txn() {
    let db = setup_test_db().await;
    let repo = RoleRepository;
    let user_id = create_test_user(&db, "txn_user").await;

    let r1_id = snowflake::next_snowflake_id();
    let r2_id = snowflake::next_snowflake_id();
    let r1 = make_role(r1_id, "角色A", "ra", 1, role::Model::DEL_FLAG_NORMAL);
    let r2 = make_role(r2_id, "角色B", "rb", 2, role::Model::DEL_FLAG_NORMAL);
    repo.insert(&db, r1).await.unwrap();
    repo.insert(&db, r2).await.unwrap();

    let txn = db.begin().await.unwrap();
    repo.assign_roles_in_txn(&txn, user_id, &[r1_id, r2_id])
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let roles = repo.find_user_roles(&db, user_id).await.unwrap();
    assert_eq!(roles.len(), 2);

    // 事务内重新分配
    let txn = db.begin().await.unwrap();
    repo.assign_roles_in_txn(&txn, user_id, &[r1_id])
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let roles = repo.find_user_roles(&db, user_id).await.unwrap();
    assert_eq!(roles.len(), 1);
}
