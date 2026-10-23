use std::process::Command;

use ryframe_application::{
    ports::tenants::{TenantMenuTemplate, TenantPermissionTemplate},
    system::platform::{
        is_business_permission, is_platform_permission, is_platform_route,
        register_business_access_scope,
    },
};

use super::{Call, actor, params, policy_template, service};

#[test]
fn business_role_defaults_and_global_boundary_are_isolated_from_other_tests() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "tenant_provisioning::business_access::business_access_policy_child",
            "--nocapture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "隔离策略验证失败：{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "子进程必须实际执行策略测试"
    );
    assert!(!is_business_permission("order:order:list"));
}

#[test]
#[ignore = "只由隔离策略父测试以精确测试名启动"]
fn business_access_policy_child() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(verify_policy());
}

async fn verify_policy() {
    let crud = ["add", "list", "edit", "remove"].map(|action| format!("order:order:{action}"));
    let global = ["add", "list", "edit", "remove"].map(|action| format!("legacy:record:{action}"));
    let all = crud
        .iter()
        .chain(&global)
        .cloned()
        .chain(["order:order:export".into()])
        .collect::<Vec<_>>();
    let routes = vec!["legacy".into(), "legacy.record".into()];
    register_business_access_scope(all.clone(), crud.clone(), global.clone(), routes.clone())
        .unwrap();
    register_business_access_scope(all, crud.clone(), global.clone(), routes).unwrap();
    assert!(
        register_business_access_scope(Vec::new(), Vec::new(), Vec::new(), Vec::new()).is_err()
    );
    assert!(is_platform_permission("LEGACY:RECORD:LIST"));
    assert!(is_platform_route("LEGACY.RECORD"));
    assert!(!is_platform_permission("order:order:list"));
    let mut template = policy_template();
    let prototype = template.authorization.permissions[0].clone();
    let extension = "order:order:export".to_string();
    for (index, code) in crud.iter().chain(&global).chain([&extension]).enumerate() {
        template
            .authorization
            .permissions
            .push(TenantPermissionTemplate {
                source_id: 100 + index as i64,
                code: code.clone(),
                parent_source_id: None,
                ..prototype.clone()
            });
    }
    let prototype = template.authorization.menus[0].clone();
    for (index, route) in ["order", "order.order", "legacy", "legacy.record"]
        .iter()
        .enumerate()
    {
        template.authorization.menus.push(TenantMenuTemplate {
            source_id: 100 + index as i64,
            route_key: Some((*route).into()),
            ..prototype.clone()
        });
    }
    let (service, state) = service(None, template, Vec::new());
    service.create(&actor(), params()).await.unwrap();
    let calls = state.calls();
    let authorization = calls
        .iter()
        .find_map(|call| match call {
            Call::Authorization(_, template) => Some(template),
            _ => None,
        })
        .unwrap();
    for code in crud {
        let permission = authorization
            .permissions
            .iter()
            .find(|permission| permission.code == code)
            .unwrap();
        assert!(permission.assign_admin);
        assert!(!permission.assign_user);
    }
    let extension = authorization
        .permissions
        .iter()
        .find(|permission| permission.code == "order:order:export")
        .unwrap();
    assert!(!extension.assign_admin && !extension.assign_user);
    assert!(
        authorization
            .permissions
            .iter()
            .all(|permission| !permission.code.starts_with("legacy:"))
    );
    assert!(authorization.menus.iter().all(|menu| {
        !menu
            .route_key
            .as_deref()
            .is_some_and(|route| route.starts_with("legacy"))
    }));
    assert!(
        authorization
            .menus
            .iter()
            .any(|menu| menu.route_key.as_deref() == Some("order.order"))
    );
}
