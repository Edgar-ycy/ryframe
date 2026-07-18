mod common;

use ryframe_common::{ActorContext, DataScope};
use ryframe_db::DatabaseCluster;
use ryframe_service::system::{CreateMenuCommand, MenuService, MenuType, RoleService};

fn actor() -> ActorContext {
    ActorContext {
        user_id: 1,
        tenant_id: "system".into(),
        username: "admin".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: true,
    }
}

#[tokio::test]
async fn role_service_constructs_without_menu_repository() {
    let _svc = RoleService::new(DatabaseCluster::single(Default::default()), None);
}

#[tokio::test]
async fn menu_service_create_only_persists_structure_fields() {
    let db = common::setup_test_db().await;
    let svc = MenuService::new(DatabaseCluster::single(db.connection().clone()), None);

    let menu = svc
        .create(
            &actor(),
            CreateMenuCommand {
                name: "系统管理".into(),
                parent_id: None,
                menu_type: MenuType::Directory,
                perm_id: None,
                route_key: Some("  test.system  ".into()),
                icon: Some("Setting".into()),
                sort: 1,
                visible: true,
            },
        )
        .await
        .unwrap();

    assert_eq!(menu.name, "系统管理");
    assert_eq!(menu.icon.as_deref(), Some("Setting"));
    assert_eq!(menu.menu_type, "M");
    assert_eq!(menu.route_key.as_deref(), Some("test.system"));
    assert_eq!(menu.sort, 1);
    assert!(menu.visible);
}
