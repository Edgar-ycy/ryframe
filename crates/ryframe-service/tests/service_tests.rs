mod common;

use ryframe_core::LoggedRepo;
use ryframe_db::{MenuRepository, PermissionRepository, RoleRepository};
use ryframe_service::system::{MenuServiceImpl, RoleServiceImpl};

#[tokio::test]
async fn role_service_constructs_without_menu_repository() {
    let _svc = RoleServiceImpl {
        role_repo: LoggedRepo::new(RoleRepository),
        perm_repo: LoggedRepo::new(PermissionRepository),
    };
}

#[tokio::test]
async fn menu_service_create_only_persists_structure_fields() {
    let db = common::setup_test_db().await;
    let svc = MenuServiceImpl {
        menu_repo: LoggedRepo::new(MenuRepository),
        redis: None,
    };

    let menu = svc
        .create(&db, "系统管理", None, "M", Some("Setting"), 1, true)
        .await
        .unwrap();

    assert_eq!(menu.name, "系统管理");
    assert_eq!(menu.icon.as_deref(), Some("Setting"));
    assert_eq!(menu.menu_type, "M");
    assert_eq!(menu.sort, 1);
    assert!(menu.visible);
}
