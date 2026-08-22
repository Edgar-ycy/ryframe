use ryframe_api::{openapi::ApiDoc, permission_catalog::*};

mod permission_catalog {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn catalog_contains_governance_permission_and_unique_sets() {
        assert!(permission_codes().contains(&"tenant:capability:override"));
        assert_eq!(
            permission_codes().len(),
            permission_codes()
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
        );
        assert_eq!(
            menu_routes().len(),
            menu_routes()
                .iter()
                .map(|menu| menu.route_key)
                .collect::<BTreeSet<_>>()
                .len()
        );
        assert_eq!(
            route_policies().len(),
            route_policies()
                .iter()
                .map(|route| (route.method, route.path))
                .collect::<BTreeSet<_>>()
                .len()
        );
    }

    #[test]
    fn generated_menu_page_keys_are_typed_and_unique() {
        let mut page_keys = BTreeSet::new();
        for menu in menu_routes() {
            match menu.menu_type {
                "M" => assert_eq!(
                    menu.page_key, None,
                    "目录菜单 {} 不得声明 page_key",
                    menu.route_key
                ),
                "C" => {
                    let page_key = menu
                        .page_key
                        .filter(|page_key| !page_key.is_empty())
                        .expect("页面菜单必须生成非空 page_key");
                    assert!(
                        page_keys.insert(page_key),
                        "页面 page_key 必须唯一: {page_key}"
                    );
                }
                menu_type => panic!("访问目录存在未知菜单类型: {menu_type}"),
            }
        }
    }

    #[test]
    fn route_policy_constraints_match_policy_kind() {
        assert!(SUPPORTED_HTTP_METHODS.contains(&"PATCH"));
        for route in route_policies() {
            assert!(SUPPORTED_HTTP_METHODS.contains(&route.method));
            match route.policy {
                AccessPolicy::Public | AccessPolicy::Authenticated => {
                    assert_eq!(route.permission_code, None);
                    assert_eq!(route.capability_code, None);
                }
                AccessPolicy::Permission => {
                    assert!(route.permission_code.is_some());
                    assert_eq!(route.capability_code, None);
                }
                AccessPolicy::Capability => assert!(route.capability_code.is_some()),
            }
        }
    }

    #[test]
    fn menu_references_are_closed() {
        let permissions = permission_codes().iter().copied().collect::<BTreeSet<_>>();
        let capabilities = capabilities()
            .iter()
            .map(|capability| capability.code)
            .collect::<BTreeSet<_>>();
        for menu in menu_routes() {
            if let Some(permission) = menu.permission_code {
                assert!(permissions.contains(permission));
            }
            if let Some(capability) = menu.capability_code {
                assert!(capabilities.contains(capability));
            }
        }
    }

    #[test]
    fn capability_catalog_matches_application_contract() {
        assert_eq!(
            capabilities().len(),
            ryframe_application::system::CAPABILITY_CATALOG.len()
        );
        for capability in capabilities() {
            let application = ryframe_application::system::CAPABILITY_CATALOG
                .iter()
                .find(|candidate| candidate.code == capability.code)
                .expect("访问目录能力必须存在于应用能力目录");
            assert_eq!(capability.route_keys, application.route_keys);
            assert_eq!(capability.permission_codes, application.permission_codes);
        }
    }
}

mod openapi {
    use std::collections::BTreeSet;

    use serde_json::Value;
    use utoipa::OpenApi;

    use super::ApiDoc;

    #[test]
    fn menu_route_extension_exactly_matches_compiled_catalog() {
        let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI 必须可序列化");
        let routes = document["x-ryframe-menu-routes"]["routes"]
            .as_array()
            .expect("OpenAPI 必须输出菜单路由数组");
        let catalog = ryframe_api::permission_catalog::menu_routes();
        assert_eq!(routes.len(), catalog.len());

        let mut page_keys = BTreeSet::new();
        for (route, menu) in routes.iter().zip(catalog) {
            assert_eq!(
                route,
                &serde_json::json!({
                    "route_key": menu.route_key,
                    "name": menu.name,
                    "title_key": menu.title_key,
                    "menu_type": menu.menu_type,
                    "page_key": menu.page_key,
                    "permission_code": menu.permission_code,
                    "capability_code": menu.capability_code,
                })
            );
            match menu.menu_type {
                "M" => assert!(route["page_key"].is_null()),
                "C" => {
                    let page_key = route["page_key"]
                        .as_str()
                        .filter(|page_key| !page_key.is_empty())
                        .expect("页面菜单必须输出非空 page_key");
                    assert!(page_keys.insert(page_key), "页面 page_key 必须唯一");
                }
                menu_type => panic!("访问目录存在未知菜单类型: {menu_type}"),
            }
        }
    }

    #[test]
    fn role_option_purpose_is_required_and_complete_in_openapi() {
        let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI 必须可序列化");
        let parameters = document["paths"]["/api/v1/system/roles/options"]["get"]["parameters"]
            .as_array()
            .expect("角色选项接口必须声明查询参数");
        let purpose = parameters
            .iter()
            .find(|parameter| parameter["name"] == "purpose")
            .expect("角色选项接口必须声明 purpose");
        assert_eq!(purpose["in"], "query");
        assert_eq!(purpose["required"], true);
        assert_eq!(
            purpose["schema"]["$ref"],
            "#/components/schemas/RoleOptionPurposeDto"
        );

        let values = document["components"]["schemas"]["RoleOptionPurposeDto"]["enum"]
            .as_array()
            .expect("角色选项用途必须声明枚举")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(values, ["user_assignment", "service_account_assignment"]);
    }
}
