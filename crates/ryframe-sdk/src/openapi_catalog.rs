//! 从业务资源事实源补齐组合后的权限与菜单契约。

use ryframe_api::openapi::OpenApiDocument;
use ryframe_kernel::{AppError, AppResult};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(crate) fn merge_business_catalog(
    document: &mut OpenApiDocument,
    fragment: &OpenApiDocument,
) -> AppResult<()> {
    let Some(resources) = fragment
        .extensions
        .as_ref()
        .and_then(|value| value.get("x-ryframe-crud-resources"))
        .and_then(|value| value.get("resources"))
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    let extensions = document.extensions.get_or_insert_default();
    let mut codes = extensions
        .get("x-ryframe-permission-catalog")
        .and_then(|value| value.get("codes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut routes = extensions
        .get("x-ryframe-menu-routes")
        .and_then(|value| value.get("routes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut keys = routes
        .iter()
        .filter_map(|value| {
            value
                .get("route_key")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>();
    for resource in resources {
        let permissions = resource
            .pointer("/access/permissions")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("权限"))?;
        for value in permissions.values() {
            if value.as_str().is_none_or(str::is_empty) {
                return Err(invalid("权限"));
            }
            codes.push(value.clone());
        }
        let route_key = text(resource, "/route/key")?;
        if !keys.insert(route_key.to_owned()) {
            return Err(invalid("重复菜单路由"));
        }
        routes.push(json!({
            "route_key": route_key,
            "name": text(resource, "/menu/labels/zh_cn")?,
            "title_key": text(resource, "/name")?,
            "menu_type": "C",
            "page_key": route_key,
            "permission_code": text(resource, "/access/permissions/list")?,
            "capability_code": resource.pointer("/access/capability").cloned().unwrap_or(Value::Null),
        }));
    }
    codes.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    codes.dedup();
    routes.sort_by(|left, right| left["route_key"].as_str().cmp(&right["route_key"].as_str()));
    extensions.insert(
        "x-ryframe-permission-catalog".into(),
        json!({"version": 1, "codes": codes}),
    );
    extensions.insert(
        "x-ryframe-menu-routes".into(),
        json!({"version": 2, "routes": routes}),
    );
    Ok(())
}

fn text<'a>(resource: &'a Value, path: &str) -> AppResult<&'a str> {
    resource
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(path))
}

fn invalid(field: &str) -> AppError {
    AppError::Config(format!("业务资源契约无效：{field}"))
}
