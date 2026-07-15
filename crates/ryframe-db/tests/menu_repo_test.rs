mod common;

use chrono::Utc;
use ryframe_core::{TenantContext, repository::Repository, with_tenant_context};
use ryframe_db::{
    MenuRepository, PermissionRepository,
    entities::{menu, permission},
};

fn make_menu(
    id: i64,
    name: &str,
    parent_id: Option<i64>,
    menu_type: &str,
    perm_id: Option<i64>,
    status: &str,
    sort: i32,
) -> menu::Model {
    let now = Utc::now();
    menu::Model {
        id,
        tenant_id: "system".into(),
        name: name.into(),
        parent_id,
        menu_type: menu_type.into(),
        perm_id,
        route_key: Some(format!("test.menu.{id}")),
        icon: None,
        sort,
        visible: true,
        status: status.into(),
        remark: None,
        del_flag: menu::Model::DEL_FLAG_NORMAL.into(),
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn find_tree_by_permission_codes_includes_matching_menus_and_ancestors() {
    with_tenant_context(
        TenantContext {
            tenant_id: "system".into(),
            is_admin: false,
        },
        async {
            let db = common::setup_test_db().await;
            let repo = MenuRepository;
            PermissionRepository
                .insert(
                    &db,
                    permission::Model {
                        id: 100,
                        tenant_id: "system".into(),
                        name: "用户查询".into(),
                        code: "system:user:list".into(),
                        parent_id: None,
                        perm_type: "api".into(),
                        icon: None,
                        sort: 1,
                        status: "1".into(),
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                    },
                )
                .await
                .unwrap();

            repo.insert(
                &db,
                make_menu(
                    1,
                    "系统管理",
                    None,
                    menu::Model::MENU_TYPE_DIR,
                    None,
                    "1",
                    1,
                ),
            )
            .await
            .unwrap();
            repo.insert(
                &db,
                make_menu(
                    4,
                    "用户管理",
                    Some(1),
                    menu::Model::MENU_TYPE_MENU,
                    Some(100),
                    "1",
                    1,
                ),
            )
            .await
            .unwrap();
            repo.insert(
                &db,
                make_menu(
                    5,
                    "角色管理",
                    Some(1),
                    menu::Model::MENU_TYPE_MENU,
                    None,
                    "1",
                    2,
                ),
            )
            .await
            .unwrap();

            let tree = repo
                .find_tree_by_permission_codes(&db, &[String::from("system:user:list")])
                .await
                .unwrap();

            assert_eq!(tree.len(), 1);
            assert_eq!(tree[0].name, "系统管理");
            assert_eq!(tree[0].children.len(), 1);
            assert_eq!(tree[0].children[0].name, "用户管理");
        },
    )
    .await;
}

#[tokio::test]
async fn find_by_permission_codes_handles_empty_and_wildcard_permissions() {
    with_tenant_context(
        TenantContext {
            tenant_id: "system".into(),
            is_admin: false,
        },
        async {
            let db = common::setup_test_db().await;
            let repo = MenuRepository;

            repo.insert(
                &db,
                make_menu(
                    6,
                    "菜单管理",
                    None,
                    menu::Model::MENU_TYPE_MENU,
                    None,
                    "1",
                    1,
                ),
            )
            .await
            .unwrap();
            repo.insert(
                &db,
                make_menu(
                    7,
                    "停用菜单",
                    None,
                    menu::Model::MENU_TYPE_MENU,
                    None,
                    "0",
                    2,
                ),
            )
            .await
            .unwrap();

            let empty = repo.find_by_permission_codes(&db, &[]).await.unwrap();
            assert!(empty.is_empty());

            let wildcard = repo
                .find_by_permission_codes(&db, &[String::from("*:*:*")])
                .await
                .unwrap();
            assert_eq!(wildcard.len(), 1);
            assert_eq!(wildcard[0].name, "菜单管理");
        },
    )
    .await;
}

#[tokio::test]
async fn button_permission_does_not_grant_parent_page_access() {
    with_tenant_context(
        TenantContext {
            tenant_id: "system".into(),
            is_admin: false,
        },
        async {
            let db = common::setup_test_db().await;
            let repo = MenuRepository;
            let permission_repo = PermissionRepository;

            for (id, name, code) in [
                (200, "菜单查询", "system:menu:list"),
                (201, "菜单新增", "system:menu:add"),
            ] {
                permission_repo
                    .insert(
                        &db,
                        permission::Model {
                            id,
                            tenant_id: "system".into(),
                            name: name.into(),
                            code: code.into(),
                            parent_id: None,
                            perm_type: "api".into(),
                            icon: None,
                            sort: 1,
                            status: "1".into(),
                            created_at: Utc::now(),
                            updated_at: Utc::now(),
                        },
                    )
                    .await
                    .unwrap();
            }

            repo.insert(
                &db,
                make_menu(
                    10,
                    "系统管理",
                    None,
                    menu::Model::MENU_TYPE_DIR,
                    None,
                    "1",
                    1,
                ),
            )
            .await
            .unwrap();
            repo.insert(
                &db,
                make_menu(
                    11,
                    "菜单管理",
                    Some(10),
                    menu::Model::MENU_TYPE_MENU,
                    Some(200),
                    "1",
                    1,
                ),
            )
            .await
            .unwrap();
            repo.insert(
                &db,
                make_menu(
                    12,
                    "菜单新增",
                    Some(11),
                    menu::Model::MENU_TYPE_BUTTON,
                    Some(201),
                    "1",
                    1,
                ),
            )
            .await
            .unwrap();

            let button_only = repo
                .find_tree_by_permission_codes(&db, &[String::from("system:menu:add")])
                .await
                .unwrap();
            assert!(button_only.is_empty(), "按钮权限不能授予父级菜单页面访问权");

            let page_and_button = repo
                .find_tree_by_permission_codes(
                    &db,
                    &[
                        String::from("system:menu:list"),
                        String::from("system:menu:add"),
                    ],
                )
                .await
                .unwrap();
            assert_eq!(page_and_button.len(), 1);
            assert_eq!(page_and_button[0].children.len(), 1);
            assert_eq!(page_and_button[0].children[0].name, "菜单管理");
            assert!(
                page_and_button[0].children[0].children.is_empty(),
                "用户菜单树不应返回按钮节点"
            );
        },
    )
    .await;
}
