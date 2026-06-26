mod common;

use chrono::Utc;
use ryframe_core::{TenantContext, repository::Repository, with_tenant_context};
use ryframe_db::{MenuRepository, entities::menu};

fn make_menu(
    id: i64,
    name: &str,
    parent_id: Option<i64>,
    menu_type: &str,
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

            repo.insert(
                &db,
                make_menu(1, "系统管理", None, menu::Model::MENU_TYPE_DIR, "1", 1),
            )
            .await
            .unwrap();
            repo.insert(
                &db,
                make_menu(4, "用户管理", Some(1), menu::Model::MENU_TYPE_MENU, "1", 1),
            )
            .await
            .unwrap();
            repo.insert(
                &db,
                make_menu(5, "角色管理", Some(1), menu::Model::MENU_TYPE_MENU, "1", 2),
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
                make_menu(6, "菜单管理", None, menu::Model::MENU_TYPE_MENU, "1", 1),
            )
            .await
            .unwrap();
            repo.insert(
                &db,
                make_menu(7, "停用菜单", None, menu::Model::MENU_TYPE_MENU, "0", 2),
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
