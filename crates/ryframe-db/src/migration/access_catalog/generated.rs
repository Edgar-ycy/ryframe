use std::collections::{BTreeMap, BTreeSet};

use sea_orm::DbErr;
use serde::Deserialize;

use super::{ACCESS_CATALOG_VERSION, GENERATED_ACCESS_CATALOG, is_permission_code};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedAccessCatalog {
    version: u32,
    #[serde(default)]
    resources: Vec<GeneratedAccessResource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedAccessResource {
    pub(super) name: String,
    module: String,
    #[serde(default)]
    capability: Option<String>,
    labels: GeneratedLabels,
    pub(super) menu: GeneratedMenu,
    route: GeneratedRoute,
    pub(super) permissions: GeneratedPermissions,
    #[serde(default)]
    extension_permissions: BTreeMap<String, String>,
}

impl GeneratedAccessResource {
    pub(super) fn permission_codes(&self) -> BTreeSet<&str> {
        self.permissions
            .values()
            .into_iter()
            .chain(self.extension_permissions.values().map(String::as_str))
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedLabels {
    pub(super) zh_cn: String,
    en: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedMenu {
    pub(super) key: String,
    pub(super) parent: String,
    pub(super) order: u32,
    #[serde(default)]
    pub(super) icon: Option<String>,
    pub(super) labels: GeneratedLabels,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedRoute {
    key: String,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneratedPermissions {
    create: String,
    read: String,
    pub(super) list: String,
    update: String,
    delete: String,
}

impl GeneratedPermissions {
    fn values(&self) -> [&str; 5] {
        [
            &self.create,
            &self.read,
            &self.list,
            &self.update,
            &self.delete,
        ]
    }
}

pub(super) fn generated_access_resources() -> Result<Vec<GeneratedAccessResource>, DbErr> {
    let mut catalog: GeneratedAccessCatalog =
        toml::from_str(GENERATED_ACCESS_CATALOG).map_err(|error| {
            DbErr::Custom(format!(
                "catalog/access.generated.toml 不是有效的生成访问目录: {error}"
            ))
        })?;
    if catalog.version != ACCESS_CATALOG_VERSION {
        return Err(DbErr::Custom(format!(
            "生成访问目录版本必须为 {ACCESS_CATALOG_VERSION}，实际为 {}",
            catalog.version
        )));
    }

    catalog
        .resources
        .sort_by(|left, right| left.name.cmp(&right.name));
    let mut names = BTreeSet::new();
    for resource in &catalog.resources {
        validate_generated_identifier("生成资源名称", &resource.name)?;
        validate_generated_identifier("生成资源模块", &resource.module)?;
        if let Some(capability) = &resource.capability {
            validate_generated_code("生成资源能力码", capability, '.')?;
        }
        validate_generated_label("生成资源中文标签", &resource.labels.zh_cn)?;
        validate_generated_label("生成资源英文标签", &resource.labels.en)?;
        if !names.insert(resource.name.as_str()) {
            return Err(DbErr::Custom(format!(
                "生成资源名称重复: {}",
                resource.name
            )));
        }
        validate_generated_identifier("生成菜单 route_key", &resource.menu.key)?;
        validate_generated_identifier("生成菜单 parent", &resource.menu.parent)?;
        validate_generated_label("生成菜单中文标签", &resource.menu.labels.zh_cn)?;
        validate_generated_label("生成菜单英文标签", &resource.menu.labels.en)?;
        if resource.menu.order == 0 {
            return Err(DbErr::Custom(format!(
                "生成菜单 {} 的 order 必须大于 0",
                resource.menu.key
            )));
        }
        if let Some(icon) = &resource.menu.icon {
            validate_generated_identifier("生成菜单 icon", icon)?;
        }
        if resource.route.key != resource.menu.key {
            return Err(DbErr::Custom(format!(
                "生成资源 {} 的 route.key 必须与 menu.key 一致",
                resource.name
            )));
        }
        validate_generated_identifier("生成页面 route_key", &resource.route.key)?;
        if !resource.route.path.starts_with('/')
            || resource.route.path.chars().any(char::is_whitespace)
        {
            return Err(DbErr::Custom(format!(
                "生成资源 {} 的前端路由必须是无空白的绝对路径",
                resource.name
            )));
        }
        for permission in resource.permission_codes() {
            if !is_permission_code(permission) || !permission.contains(':') {
                return Err(DbErr::Custom(format!("生成权限码格式无效: {permission:?}")));
            }
        }
        for key in resource.extension_permissions.keys() {
            validate_generated_identifier("生成扩展权限键", key)?;
        }
    }
    Ok(catalog.resources)
}

fn validate_generated_identifier(label: &str, value: &str) -> Result<(), DbErr> {
    if value.trim() != value
        || value.is_empty()
        || value.chars().any(|character| {
            !character.is_ascii_alphanumeric()
                && character != '.'
                && character != '-'
                && character != '_'
        })
    {
        return Err(DbErr::Custom(format!("{label}格式无效: {value:?}")));
    }
    Ok(())
}

fn validate_generated_code(label: &str, value: &str, separator: char) -> Result<(), DbErr> {
    if value.trim() != value
        || value.is_empty()
        || !value.contains(separator)
        || value.chars().any(char::is_whitespace)
    {
        return Err(DbErr::Custom(format!("{label}格式无效: {value:?}")));
    }
    Ok(())
}

fn validate_generated_label(label: &str, value: &str) -> Result<(), DbErr> {
    if value.trim() != value || value.is_empty() || value.chars().count() > 64 {
        return Err(DbErr::Custom(format!("{label}格式无效")));
    }
    Ok(())
}
