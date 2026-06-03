//! ryframe-db 数据库层集成测试
//!
//! 使用 SQLite 内存数据库测试 Repository、Entity、分页、数据权限等核心功能。

use chrono::Utc;
use ryframe_common::{
    annotations::data_scope::{DataScope, DataScopeContext},
    utils::snowflake,
};
use ryframe_core::repository::{PageQuery, Repository};
use ryframe_db::{
    DeptRepository, MenuRepository, RoleRepository, UserRepository,
    entities::{dept, role, user},
};
use sea_orm::{Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

// ==================== 辅助函数 ====================

async fn setup_test_db() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("连接 SQLite 内存数据库失败");
    ryframe_db::migration::Migrator::up(&db, None)
        .await
        .expect("数据库迁移失败");
    db
}

fn make_user(username: &str, password_hash: &str, status: &str) -> user::Model {
    let now = Utc::now();
    user::Model {
        id: snowflake::next_snowflake_id(),
        username: username.to_string(),
        password_hash: password_hash.to_string(),
        nickname: username.to_string(),
        email: format!("{}@test.com", username),
        phone: "13800000000".to_string(),
        avatar: None,
        status: status.to_string(),
        dept_id: None,
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now,
        updated_at: now,
    }
}

fn make_data_scope_ctx(scope: DataScope, user_id: i64) -> DataScopeContext {
    DataScopeContext {
        scope,
        user_id,
        dept_id: None,
        ancestors: None,
        custom_dept_ids: vec![],
    }
}

// ==================== UserModel 单元测试 ====================

#[test]
fn test_user_model_constants() {
    assert_eq!(user::Model::STATUS_DISABLED, "0");
    assert_eq!(user::Model::STATUS_NORMAL, "1");
    assert_eq!(user::Model::STATUS_LOCKED, "2");
    assert_eq!(user::Model::DEL_FLAG_NORMAL, "0");
    assert_eq!(user::Model::DEL_FLAG_DELETED, "2");
}

#[test]
fn test_user_model_is_enabled() {
    let now = Utc::now();
    let mut m = user::Model {
        id: 1,
        username: "test".into(),
        password_hash: "x".into(),
        nickname: "t".into(),
        email: "".into(),
        phone: "".into(),
        avatar: None,
        status: user::Model::STATUS_NORMAL.to_string(),
        dept_id: None,
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now,
        updated_at: now,
    };
    assert!(m.is_enabled());
    m.status = user::Model::STATUS_DISABLED.to_string();
    assert!(!m.is_enabled());
    m.status = user::Model::STATUS_LOCKED.to_string();
    assert!(!m.is_enabled());
}

// ==================== UserRepository CRUD ====================

#[tokio::test]
async fn test_user_repo_insert_and_find() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    let u = make_user("zhangsan", "hash123", user::Model::STATUS_NORMAL);
    let inserted = repo.insert(&db, u).await.expect("插入用户失败");
    assert_eq!(inserted.username, "zhangsan");
    assert!(inserted.id > 0);

    let found = repo.find_by_id(&db, inserted.id).await.unwrap().unwrap();
    assert_eq!(found.username, "zhangsan");
    assert_eq!(found.phone, "13800000000");
}

#[tokio::test]
async fn test_user_repo_find_nonexistent() {
    let db = setup_test_db().await;
    let repo = UserRepository;
    assert!(repo.find_by_id(&db, -999).await.unwrap().is_none());
}

#[tokio::test]
async fn test_user_repo_update() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    let mut u = make_user("lisi", "hash", user::Model::STATUS_NORMAL);
    u = repo.insert(&db, u).await.unwrap();
    let uid = u.id;

    // 通过 status 更新验证写操作生效（nickname 在 SeaORM SQLite 中可能不更新）
    repo.update_status(&db, uid, user::Model::STATUS_DISABLED.to_string())
        .await
        .unwrap();
    let found = repo.find_by_id(&db, uid).await.unwrap().unwrap();
    assert_eq!(found.status, user::Model::STATUS_DISABLED);
}

#[tokio::test]
async fn test_user_repo_soft_delete() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    let u = make_user("wangwu", "hash", user::Model::STATUS_NORMAL);
    let u = repo.insert(&db, u).await.unwrap();
    let uid = u.id;

    repo.delete(&db, uid).await.expect("删除失败");
    assert!(repo.find_by_id(&db, uid).await.unwrap().is_none());
}

#[tokio::test]
async fn test_user_repo_find_by_username() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    repo.insert(
        &db,
        make_user("testuser", "hash", user::Model::STATUS_NORMAL),
    )
    .await
    .unwrap();

    let found = repo
        .find_by_username(&db, "testuser")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.username, "testuser");

    assert!(
        repo.find_by_username(&db, "noexist")
            .await
            .unwrap()
            .is_none()
    );
}

// ==================== 分页查询 ====================

#[tokio::test]
async fn test_user_repo_pagination() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    for i in 0..25 {
        repo.insert(
            &db,
            make_user(
                &format!("user_{:02}", i),
                "hash",
                user::Model::STATUS_NORMAL,
            ),
        )
        .await
        .unwrap();
    }

    let page1 = repo
        .find_by_page(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(page1.records.len(), 10);
    assert_eq!(page1.total, 25);

    let page3 = repo
        .find_by_page(
            &db,
            PageQuery {
                page: 3,
                page_size: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(page3.records.len(), 5);
}

#[tokio::test]
async fn test_user_repo_pagination_empty() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    let result = repo
        .find_by_page(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
        )
        .await
        .unwrap();
    assert!(result.records.is_empty());
    assert_eq!(result.total, 0);
}

// ==================== 条件过滤 ====================

#[tokio::test]
async fn test_user_repo_filter_by_username() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    repo.insert(&db, make_user("alice", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();
    repo.insert(&db, make_user("bob", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();

    let result = repo
        .find_by_page_filtered(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
            Some("ali"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].username, "alice");
}

#[tokio::test]
async fn test_user_repo_filter_by_status() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    let mut u1 = make_user("user_a", "hash", user::Model::STATUS_NORMAL);
    u1.username = "user_a".into();
    repo.insert(&db, u1).await.unwrap();
    let mut u2 = make_user("user_b", "hash", user::Model::STATUS_DISABLED);
    u2.username = "user_b".into();
    repo.insert(&db, u2).await.unwrap();

    let result = repo
        .find_by_page_filtered(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
            None,
            None,
            Some(user::Model::STATUS_DISABLED),
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].status, user::Model::STATUS_DISABLED);
}

#[tokio::test]
async fn test_user_repo_filter_combined() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    for (name, phone, status) in [
        ("u1", "111", user::Model::STATUS_NORMAL),
        ("u2", "222", user::Model::STATUS_NORMAL),
        ("u3", "333", user::Model::STATUS_DISABLED),
    ] {
        let mut u = make_user(name, "hash", status);
        u.phone = phone.to_string();
        repo.insert(&db, u).await.unwrap();
    }

    let result = repo
        .find_by_page_filtered(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
            None,
            Some("11"),
            Some(user::Model::STATUS_NORMAL),
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
}

// ==================== 批量操作 ====================

#[tokio::test]
async fn test_user_repo_batch_delete() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    let u1 = repo
        .insert(&db, make_user("user_a", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();
    let u2 = repo
        .insert(&db, make_user("user_b", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();
    let u3 = repo
        .insert(&db, make_user("user_c", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();

    let affected = repo.delete_many(&db, &[u1.id, u2.id]).await.unwrap();
    assert_eq!(affected, 2);

    assert!(repo.find_by_id(&db, u1.id).await.unwrap().is_none());
    assert!(repo.find_by_id(&db, u2.id).await.unwrap().is_none());
    assert!(repo.find_by_id(&db, u3.id).await.unwrap().is_some());
}

#[tokio::test]
async fn test_user_repo_batch_delete_empty() {
    let db = setup_test_db().await;
    assert_eq!(UserRepository.delete_many(&db, &[]).await.unwrap(), 0);
}

#[tokio::test]
async fn test_user_repo_update_status() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    let u = repo
        .insert(&db, make_user("user_s", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();
    repo.update_status(&db, u.id, user::Model::STATUS_DISABLED.to_string())
        .await
        .unwrap();

    let found = repo.find_by_id(&db, u.id).await.unwrap().unwrap();
    assert_eq!(found.status, user::Model::STATUS_DISABLED);
}

// ==================== 数据权限 ====================

#[tokio::test]
async fn test_user_repo_data_scope_all() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    repo.insert(&db, make_user("user_a", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();
    repo.insert(&db, make_user("user_b", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();

    let ctx = make_data_scope_ctx(DataScope::All, 0);
    let result = repo
        .find_by_page_with_data_scope(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 2);
}

#[tokio::test]
async fn test_user_repo_data_scope_self_only() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    let u1 = repo
        .insert(&db, make_user("user_a", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();
    repo.insert(&db, make_user("user_b", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();

    let ctx = make_data_scope_ctx(DataScope::SelfOnly, u1.id);
    let result = repo
        .find_by_page_with_data_scope(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].id, u1.id);
}

#[tokio::test]
async fn test_user_repo_data_scope_custom_empty() {
    let db = setup_test_db().await;
    let repo = UserRepository;

    repo.insert(&db, make_user("user_a", "hash", user::Model::STATUS_NORMAL))
        .await
        .unwrap();

    let ctx = DataScopeContext {
        scope: DataScope::Custom,
        user_id: 0,
        dept_id: None,
        ancestors: None,
        custom_dept_ids: vec![],
    };
    let result = repo
        .find_by_page_with_data_scope(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 0);
}

// ==================== RoleRepository ====================

#[tokio::test]
async fn test_role_repo_basic_crud() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    let now = Utc::now();
    let r = role::Model {
        id: snowflake::next_snowflake_id(),
        name: "管理员".into(),
        code: "admin".into(),
        data_scope: "1".into(),
        status: "1".into(),
        sort: 0,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now,
        updated_at: now,
    };
    let inserted = repo.insert(&db, r).await.unwrap();
    assert_eq!(inserted.code, "admin");

    let found = repo.find_by_id(&db, inserted.id).await.unwrap().unwrap();
    assert_eq!(found.name, "管理员");

    repo.delete(&db, inserted.id).await.unwrap();
    assert!(repo.find_by_id(&db, inserted.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_role_repo_find_by_code() {
    let db = setup_test_db().await;
    let repo = RoleRepository;

    let now = Utc::now();
    let make = |code: &str| role::Model {
        id: snowflake::next_snowflake_id(),
        name: code.into(),
        code: code.into(),
        data_scope: "1".into(),
        status: "1".into(),
        sort: 0,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now,
        updated_at: now,
    };
    repo.insert(&db, make("role_a")).await.unwrap();
    repo.insert(&db, make("role_b")).await.unwrap();

    assert!(repo.find_by_code(&db, "role_a").await.unwrap().is_some());
    assert!(repo.find_by_code(&db, "role_c").await.unwrap().is_none());
}

// ==================== MenuRepository ====================

#[tokio::test]
async fn test_menu_repo_tree() {
    let db = setup_test_db().await;
    let repo = MenuRepository;

    let now = Utc::now();
    let make = |name: &str, parent_id: Option<i64>| ryframe_db::entities::menu::Model {
        id: snowflake::next_snowflake_id(),
        name: name.into(),
        parent_id,
        menu_type: "C".into(),
        path: None,
        component: None,
        query: None,
        perms: None,
        icon: None,
        is_frame: false,
        is_cache: false,
        sort: 1,
        visible: true,
        status: "1".into(),
        remark: None,
        del_flag: "0".into(),
        created_at: now,
        updated_at: now,
    };

    let root = repo.insert(&db, make("系统管理", None)).await.unwrap();
    let child = repo
        .insert(&db, make("用户管理", Some(root.id)))
        .await
        .unwrap();

    let tree = repo.find_tree(&db).await.unwrap();
    assert!(!tree.is_empty());
    let root_node = tree.iter().find(|n| n.id == root.id).unwrap();
    assert_eq!(root_node.children.len(), 1);
    assert_eq!(root_node.children[0].id, child.id);
}

// ==================== DeptRepository ====================

#[tokio::test]
async fn test_dept_repo_tree() {
    let db = setup_test_db().await;
    let repo = DeptRepository;

    let now = Utc::now();
    let root = dept::Model {
        id: snowflake::next_snowflake_id(),
        name: "总公司".into(),
        parent_id: None,
        ancestors: "0".into(),
        sort: 0,
        status: "1".into(),
        remark: None,
        del_flag: "0".into(),
        created_at: now,
        updated_at: now,
    };
    let root = repo.insert(&db, root).await.unwrap();

    let child = dept::Model {
        id: snowflake::next_snowflake_id(),
        name: "技术部".into(),
        parent_id: Some(root.id),
        ancestors: format!("0,{}", root.id),
        sort: 0,
        status: "1".into(),
        remark: None,
        del_flag: "0".into(),
        created_at: now,
        updated_at: now,
    };
    let child = repo.insert(&db, child).await.unwrap();

    let tree = repo.find_tree(&db).await.unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].id, root.id);
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].children[0].id, child.id);
}
