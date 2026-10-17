//! 业务访问目录与数据库初始化共用事实；HTTP 片段只负责证明接口契约一致。

use std::collections::BTreeSet;

use ryframe_api::openapi::OpenApiDocument;
use ryframe_db::migration::{BusinessAccessCatalog, compile_business_access_catalog};
use ryframe_kernel::{AppError, AppResult};
use serde_json::{Value, json};

use crate::{ResourceDatabase, RyFrameBusinessModule};

pub(crate) fn compile(modules: &[RyFrameBusinessModule]) -> AppResult<BusinessAccessCatalog> {
    let access = compile_business_access_catalog(
        modules
            .iter()
            .map(|module| (module.name(), module.access_catalog())),
    )
    .map_err(|error| AppError::Config(error.to_string()))?;
    for module in modules {
        if access
            .resources()
            .filter(|resource| resource.module == module.name())
            .count()
            != module.resources().len()
        {
            return Err(invalid(
                "业务模型与访问目录数量不一致；运行 cargo generate -- resource --write",
            ));
        }
        for resource in module.resources() {
            let binding = access
                .resource(module.name(), resource.name)
                .ok_or_else(|| invalid("业务模型缺少访问目录"))?;
            if binding.system_tenant_only != (resource.database == ResourceDatabase::Control) {
                return Err(invalid("业务模型与访问目录的租户边界不一致"));
            }
        }
    }
    Ok(access)
}

pub(crate) fn validate_fragment(
    module: &RyFrameBusinessModule,
    fragment: &OpenApiDocument,
    access: &BusinessAccessCatalog,
) -> AppResult<()> {
    let rows = fragment
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("x-ryframe-crud-resources"))
        .and_then(|value| value.get("resources"))
        .and_then(Value::as_array);
    let mut seen = BTreeSet::new();
    for row in rows.into_iter().flatten() {
        let name = text(row, "/name")?;
        let binding = access
            .resource(module.name(), name)
            .ok_or_else(|| invalid("HTTP 片段包含未登记的业务资源"))?;
        if !seen.insert(name) {
            return Err(invalid("HTTP 片段重复声明业务资源"));
        }
        let permissions = row
            .pointer("/access/permissions")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("业务权限"))?;
        if permissions.len() != 5
            || row.get("extension_permissions") != Some(&json!(binding.extension_permissions))
        {
            return Err(invalid("HTTP 权限集合与生成访问目录不一致"));
        }
        for (index, action) in ["create", "read", "list", "update", "delete"]
            .iter()
            .enumerate()
        {
            if permissions.get(*action).and_then(Value::as_str)
                != Some(binding.permissions[index].as_str())
            {
                return Err(invalid("HTTP 权限与生成访问目录不一致"));
            }
        }
        let menu = access
            .menus()
            .iter()
            .find(|menu| menu.route_key == binding.route_key)
            .ok_or_else(|| invalid("业务页面缺少菜单"))?;
        let system_only = match row.pointer("/access/system_tenant_only") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => return Err(invalid("HTTP 业务租户边界必须是布尔值")),
        };
        if text(row, "/route/key")? != binding.route_key
            || text(row, "/route/path")? != binding.route_path
            || text(row, "/menu/key")? != binding.route_key
            || text(row, "/menu/parent")? != binding.parent
            || text(row, "/menu/labels/zh_cn")? != menu.name
            || system_only != binding.system_tenant_only
        {
            return Err(invalid("HTTP 菜单、路由或租户边界与生成访问目录不一致"));
        }
    }
    if seen.len() != module.resources().len() {
        return Err(invalid("HTTP 片段未包含完整业务资源目录"));
    }
    Ok(())
}

pub(crate) fn merge_business_catalog(
    document: &mut OpenApiDocument,
    access: &BusinessAccessCatalog,
) -> AppResult<()> {
    if access.resources().next().is_none() {
        return Ok(());
    }
    let extensions = document.extensions.get_or_insert_default();
    let mut codes = extensions
        .get("x-ryframe-permission-catalog")
        .and_then(|value| value.get("codes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    codes.extend(
        access
            .permission_names()
            .keys()
            .map(|code| Value::String(code.clone())),
    );
    let mut routes = extensions
        .get("x-ryframe-menu-routes")
        .and_then(|value| value.get("routes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut keys = routes
        .iter()
        .filter_map(|route| {
            route
                .get("route_key")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>();
    let mut titles = routes
        .iter()
        .filter_map(|route| {
            route
                .get("title_key")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>();
    for menu in access.menus() {
        if !keys.insert(menu.route_key.clone()) {
            return Err(invalid("业务菜单路由与当前 HTTP 目录冲突"));
        }
        let title = if menu.menu_type == "M" {
            format!(
                "{}BusinessDirectory",
                title_key(&menu.route_key.replace('-', "_"))?
            )
        } else {
            let resource = access
                .resources()
                .find(|resource| resource.route_key == menu.route_key)
                .ok_or_else(|| invalid("业务页面缺少资源"))?;
            title_key(&resource.resource)?
        };
        if !titles.insert(title.clone()) {
            return Err(AppError::Config(format!(
                "业务菜单标题键冲突：{title}；为业务资源选择不同名称后重新生成"
            )));
        }
        routes.push(json!({
            "route_key": menu.route_key, "name": menu.name, "title_key": title,
            "menu_type": menu.menu_type, "icon": menu.icon(),
            "page_key": (menu.menu_type == "C").then_some(&menu.route_key),
            "permission_code": menu.permission, "capability_code": Value::Null,
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
        json!({"version": 3, "routes": routes}),
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

fn title_key(name: &str) -> AppResult<String> {
    if !name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        || name.split('_').any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
    {
        return Err(invalid("资源名称"));
    }
    let mut parts = name.split('_');
    let mut key = parts.next().ok_or_else(|| invalid("资源名称"))?.to_owned();
    for part in parts {
        let mut bytes = part.bytes();
        key.push(char::from(
            bytes
                .next()
                .ok_or_else(|| invalid("资源名称"))?
                .to_ascii_uppercase(),
        ));
        key.extend(bytes.map(char::from));
    }
    Ok(key)
}

fn invalid(field: &str) -> AppError {
    AppError::Config(format!("业务资源契约无效：{field}"))
}
