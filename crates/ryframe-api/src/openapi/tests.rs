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
    let catalog = crate::permission_catalog::menu_routes();
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
