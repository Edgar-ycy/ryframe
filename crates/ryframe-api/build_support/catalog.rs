use std::{collections::BTreeSet, error::Error, fs, path::Path};

use super::{
    model::{AccessCatalog, CATALOG_VERSION, GeneratedAccessCatalog, GeneratedResource, MenuEntry},
    validation::{validate_catalog, validate_code, validate_identifier},
};

pub(super) fn load_catalog(
    catalog_path: &Path,
    generated_catalog_path: &Path,
) -> Result<AccessCatalog, Box<dyn Error>> {
    let catalog_source = fs::read_to_string(catalog_path)?;
    let mut catalog: AccessCatalog = toml::from_str(&catalog_source)
        .map_err(|error| format!("{} 不是有效的访问目录: {error}", catalog_path.display()))?;
    let generated_catalog_source = fs::read_to_string(generated_catalog_path)?;
    let generated_catalog: GeneratedAccessCatalog = toml::from_str(&generated_catalog_source)
        .map_err(|error| {
            format!(
                "{} 不是有效的生成访问目录: {error}",
                generated_catalog_path.display()
            )
        })?;
    merge_generated_catalog(&mut catalog, generated_catalog)?;
    validate_catalog(&catalog)?;
    Ok(catalog)
}

fn merge_generated_catalog(
    catalog: &mut AccessCatalog,
    mut generated: GeneratedAccessCatalog,
) -> Result<(), Box<dyn Error>> {
    if generated.version != CATALOG_VERSION {
        return Err(format!(
            "生成访问目录版本必须为 {CATALOG_VERSION}，实际为 {}",
            generated.version
        )
        .into());
    }

    generated
        .resources
        .sort_by(|left, right| left.name.cmp(&right.name));
    let mut generated_names = BTreeSet::new();
    let mut generated_permissions = BTreeSet::new();
    let mut generated_menu_keys = BTreeSet::new();
    let manual_permissions = catalog.permissions.iter().cloned().collect::<BTreeSet<_>>();
    let manual_menu_keys = catalog
        .menus
        .iter()
        .map(|menu| menu.route_key.clone())
        .collect::<BTreeSet<_>>();

    for resource in generated.resources {
        validate_identifier("生成资源名称", &resource.name)?;
        validate_identifier("生成资源模块", &resource.module)?;
        if let Some(capability) = &resource.capability {
            validate_code("生成资源能力码", capability, '.')?;
        }
        validate_generated_label("生成资源中文标签", &resource.labels.zh_cn)?;
        validate_generated_label("生成资源英文标签", &resource.labels.en)?;
        if !generated_names.insert(resource.name.clone()) {
            return Err(format!("生成资源名称重复: {}", resource.name).into());
        }

        validate_generated_menu(&resource)?;
        if !manual_menu_keys.contains(&resource.menu.parent) {
            return Err(format!(
                "生成菜单 {} 的父菜单 {} 不存在于手写访问目录",
                resource.menu.key, resource.menu.parent
            )
            .into());
        }
        if manual_menu_keys.contains(&resource.menu.key)
            || !generated_menu_keys.insert(resource.menu.key.clone())
        {
            return Err(format!("生成菜单 route_key 冲突: {}", resource.menu.key).into());
        }

        let resource_permissions = resource
            .permissions
            .values()
            .into_iter()
            .chain(resource.extension_permissions.values().map(String::as_str))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        for permission in &resource_permissions {
            validate_code("生成权限码", permission, ':')?;
            if manual_permissions.contains(permission)
                || !generated_permissions.insert(permission.clone())
            {
                return Err(format!("生成权限码与其他资源或手写目录冲突: {permission}").into());
            }
        }
        catalog.permissions.extend(resource_permissions);
        let list_permission = resource.permissions.list.clone();
        catalog.menus.push(MenuEntry {
            icon: resource.menu.icon,
            route_key: resource.menu.key,
            order: resource.menu.order,
            name: resource.menu.labels.zh_cn,
            title_key: resource.name,
            menu_type: "C".to_owned(),
            page_key: Some(resource.route.key),
            permission: Some(list_permission),
            capability: resource.capability.clone(),
        });
    }

    catalog.permissions.sort();
    catalog
        .menus
        .sort_by(|left, right| left.route_key.cmp(&right.route_key));
    Ok(())
}

fn validate_generated_label(label: &str, value: &str) -> Result<(), Box<dyn Error>> {
    if value.trim() != value || value.is_empty() || value.chars().count() > 64 {
        Err(format!("{label}格式无效").into())
    } else {
        Ok(())
    }
}

fn validate_generated_menu(resource: &GeneratedResource) -> Result<(), Box<dyn Error>> {
    validate_identifier("生成菜单 route_key", &resource.menu.key)?;
    validate_identifier("生成菜单 parent", &resource.menu.parent)?;
    if let Some(icon) = resource.menu.icon.as_deref() {
        validate_identifier("生成菜单 icon", icon)?;
    }
    validate_generated_label("生成菜单中文标签", &resource.menu.labels.zh_cn)?;
    validate_generated_label("生成菜单英文标签", &resource.menu.labels.en)?;
    if resource.menu.order == 0 {
        return Err(format!("生成菜单 {} 的 order 必须大于 0", resource.menu.key).into());
    }
    validate_identifier("生成页面 route_key", &resource.route.key)?;
    if !resource.route.path.starts_with('/') || resource.route.path.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "生成资源 {} 的前端路由必须是无空白的绝对路径",
            resource.name
        )
        .into());
    }
    Ok(())
}
