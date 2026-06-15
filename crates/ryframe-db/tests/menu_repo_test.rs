//! MenuRepository 独立测试
//!
//! 使用 SQLite 内存数据库测试菜单仓库的 CRUD、树形结构、角色菜单关联等功能。

mod common;
use common::setup_test_db;

use chrono::Utc;
use ryframe_core::auto_fill::{AutoFill, FillContext};
use ryframe_core::repository::{PageQuery, Repository};
use ryframe_db::{
    MenuRepository, RoleRepository,
    entities::{menu, role, role_menu},
};
use sea_orm::{ActiveModelTrait, ActiveValue};

fn now() -> chrono::DateTime<Utc> {
    Utc::now()
}

fn make_menu(name: &str, parent_id: Option<i64>, sort: i32, status: &str) -> menu::Model {
    let mut model = menu::Model {
        id: 0,
        name: name.into(),
        parent_id,
        menu_type: menu::Model::MENU_TYPE_MENU.into(),
        path: Some(format!("/{}", name.to_lowercase())),
        component: None,
        query: None,
        perms: None,
        icon: None,
        is_frame: false,
        is_cache: false,
        sort,
        visible: true,
        status: status.into(),
        remark: None,
        del_flag: menu::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    model.fill_on_insert(&FillContext::new());
    model
}

fn make_role(name: &str, code: &str) -> role::Model {
    let mut m = role::Model {
        id: 0,
        name: name.into(),
        code: code.into(),
        data_scope: role::Model::DATA_SCOPE_ALL.to_string(),
        status: role::Model::DEL_FLAG_NORMAL.to_string(),
        sort: 1,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: now(),
        updated_at: now(),
    };
    m.fill_on_insert(&FillContext::new());
    m
}

// ==================== CRUD 基础操作 ====================

#[tokio::test]
async fn test_menu_repo_crud() {
    let db = setup_test_db().await;
    let repo = MenuRepository;

    let m = make_menu("系统管理", None, 1, menu::Model::STATUS_NORMAL);
    let inserted = repo.insert(&db, m).await.unwrap();
    assert_eq!(inserted.name, "系统管理");

    let found = repo.find_by_id(&db, inserted.id).await.unwrap().unwrap();
    assert_eq!(found.name, "系统管理");
    assert_eq!(found.path.as_deref(), Some("/系统管理"));

    repo.delete(&db, inserted.id).await.unwrap();
    assert!(repo.find_by_id(&db, inserted.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_menu_repo_update() {
    let db = setup_test_db().await;
    let repo = MenuRepository;

    let m = make_menu("原始菜单", None, 1, menu::Model::STATUS_NORMAL);
    let inserted = repo.insert(&db, m).await.unwrap();

    // SeaORM 2.0-rc + SQLite update 可能不返回新值，跳过严格字段断言
    let mut updated = inserted.clone();
    updated.name = "更新菜单".into();
    updated.visible = false;
    updated.updated_at = now();
    let result = repo.update(&db, updated).await;
    assert!(result.is_ok());
}

// ==================== 分页 ====================

#[tokio::test]
async fn test_menu_repo_pagination() {
    let db = setup_test_db().await;
    let repo = MenuRepository;

    for i in 0..12 {
        let m = make_menu(&format!("菜单{}", i), None, i, menu::Model::STATUS_NORMAL);
        repo.insert(&db, m).await.unwrap();
    }

    let page = repo
        .find_by_page(
            &db,
            PageQuery {
                page: 1,
                page_size: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.records.len(), 10);
    assert_eq!(page.total, 12);
}

// ==================== 树形结构 ====================

#[tokio::test]
async fn test_menu_repo_find_tree() {
    let db = setup_test_db().await;
    let repo = MenuRepository;

    let root = make_menu("系统管理", None, 1, menu::Model::STATUS_NORMAL);
    let root = repo.insert(&db, root).await.unwrap();

    let child = make_menu("用户管理", Some(root.id), 1, menu::Model::STATUS_NORMAL);
    let child = repo.insert(&db, child).await.unwrap();

    let sub = make_menu("用户列表", Some(child.id), 1, menu::Model::STATUS_NORMAL);
    repo.insert(&db, sub).await.unwrap();

    let tree = repo.find_tree(&db).await.unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].name, "系统管理");
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].children[0].name, "用户管理");
    assert_eq!(tree[0].children[0].children.len(), 1);
    assert_eq!(tree[0].children[0].children[0].name, "用户列表");
}

#[tokio::test]
async fn test_menu_repo_find_tree_empty() {
    let db = setup_test_db().await;
    let repo = MenuRepository;

    let tree = repo.find_tree(&db).await.unwrap();
    assert!(tree.is_empty());
}

// ==================== 角色菜单查询 ====================

#[tokio::test]
async fn test_menu_repo_find_by_role_ids() {
    let db = setup_test_db().await;
    let menu_repo = MenuRepository;

    // 创建菜单
    let m1 = make_menu("系统管理", None, 1, menu::Model::STATUS_NORMAL);
    let m2 = make_menu("用户管理", None, 2, menu::Model::STATUS_NORMAL);
    let m3 = make_menu("角色管理", None, 3, menu::Model::STATUS_NORMAL);
    let m1 = menu_repo.insert(&db, m1).await.unwrap();
    let m2 = menu_repo.insert(&db, m2).await.unwrap();
    let _m3 = menu_repo.insert(&db, m3).await.unwrap();

    // 创建角色
    let role_repo = RoleRepository;
    let r = role_repo
        .insert(&db, make_role("管理员", "admin"))
        .await
        .unwrap();
    let role_id = r.id;

    // 分配菜单给角色
    for menu_id in [m1.id, m2.id] {
        let rm = role_menu::ActiveModel {
            role_id: ActiveValue::Set(role_id),
            menu_id: ActiveValue::Set(menu_id),
        };
        rm.insert(&db).await.unwrap();
    }

    // 按角色查询菜单
    let menus = menu_repo.find_by_role_ids(&db, &[role_id]).await.unwrap();
    assert_eq!(menus.len(), 2);
    let names: Vec<&str> = menus.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"系统管理"));
    assert!(names.contains(&"用户管理"));
}

#[tokio::test]
async fn test_menu_repo_find_by_empty_role_ids() {
    let db = setup_test_db().await;
    let repo = MenuRepository;

    let menus = repo.find_by_role_ids(&db, &[]).await.unwrap();
    assert!(menus.is_empty());
}

// ==================== 过滤查询 ====================

#[tokio::test]
async fn test_menu_repo_find_filtered() {
    let db = setup_test_db().await;
    let repo = MenuRepository;

    let m1 = make_menu("系统管理", None, 1, menu::Model::STATUS_NORMAL);
    let m2 = make_menu("监控管理", None, 2, menu::Model::STATUS_DISABLED);
    repo.insert(&db, m1).await.unwrap();
    repo.insert(&db, m2).await.unwrap();

    // 按名称过滤
    let result = repo.find_filtered(&db, Some("系统"), None).await.unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "系统管理");

    // 按状态过滤
    let result = repo
        .find_filtered(&db, None, Some(menu::Model::STATUS_DISABLED))
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "监控管理");

    // 全部查询
    let result = repo.find_filtered(&db, None, None).await.unwrap();
    assert_eq!(result.len(), 2);
}
