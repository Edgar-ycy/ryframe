use std::collections::{BTreeMap, BTreeSet};

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, TryGetable};
use serde::Deserialize;

const ACCESS_CATALOG: &str = include_str!("../../../../catalog/access.toml");
const GENERATED_ACCESS_CATALOG: &str = include_str!("../../../../catalog/access.generated.toml");
const ACCESS_CATALOG_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessMenu {
    pub route_key: String,
    pub name: String,
    pub menu_type: String,
    pub permission: Option<String>,
    parent_route_key: Option<String>,
    sort: i32,
}

impl AccessMenu {
    pub fn parent_route_key(&self) -> Option<&str> {
        self.parent_route_key.as_deref()
    }

    pub const fn sort(&self) -> i32 {
        self.sort
    }
}

pub(super) async fn seed_access_catalog<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let permissions = access_permission_codes()?;
    let permission_names = access_permission_names()?;
    for (index, code) in permissions.iter().enumerate() {
        let index = i32::try_from(index)
            .map_err(|_| DbErr::Custom("访问目录权限数量超出基线可表示范围".into()))?;
        let name = permission_names
            .get(code)
            .map(String::as_str)
            .unwrap_or(code.as_str());
        if permission_id(db, code).await?.is_some() {
            db.execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "UPDATE `sys_permission` SET `name` = IF(`name` = `code`, ?, `name`) \
                 WHERE `tenant_id` = 'system' AND `code` = ?",
                [name.into(), code.as_str().into()],
            ))
            .await?;
            continue;
        }
        let id = next_permission_id(db).await?;
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO `sys_permission` \
             (`id`, `tenant_id`, `name`, `code`, `parent_id`, `perm_type`, `icon`, `sort`, `status`, `created_at`, `updated_at`) \
             VALUES (?, 'system', ?, ?, NULL, 'api', NULL, ?, '1', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
            [id.into(), name.into(), code.as_str().into(), index.into()],
        ))
        .await?;
    }

    for menu in &access_menus()? {
        let permission_id = match menu.permission.as_deref() {
            Some(code) => permission_id(db, code).await?,
            None => None,
        };
        let parent_id = match menu.parent_route_key() {
            Some(route_key) => menu_id(db, route_key).await?,
            None => None,
        };
        if let Some(id) = menu_id(db, &menu.route_key).await? {
            db.execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "UPDATE `sys_menu` SET `name` = IF(`name` = `route_key`, ?, `name`), \
                 `parent_id` = ?, `menu_type` = ?, `perm_id` = ?, `sort` = ?, `status` = '1', \
                 `del_flag` = '0', `updated_at` = UTC_TIMESTAMP(6) \
                 WHERE `id` = ? AND `tenant_id` = 'system'",
                [
                    menu.name.as_str().into(),
                    parent_id.into(),
                    menu.menu_type.as_str().into(),
                    permission_id.into(),
                    menu.sort().into(),
                    id.into(),
                ],
            ))
            .await?;
            continue;
        }
        let id = next_menu_id(db).await?;
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO `sys_menu` \
             (`id`, `tenant_id`, `name`, `parent_id`, `menu_type`, `perm_id`, `route_key`, `icon`, `sort`, `visible`, `status`, `remark`, `del_flag`, `created_at`, `updated_at`) \
             VALUES (?, 'system', ?, ?, ?, ?, ?, NULL, ?, 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
            [
                id.into(),
                menu.name.as_str().into(),
                parent_id.into(),
                menu.menu_type.as_str().into(),
                permission_id.into(),
                menu.route_key.as_str().into(),
                menu.sort().into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

async fn permission_id<C>(db: &C, code: &str) -> Result<Option<i64>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT `id` FROM `sys_permission` \
             WHERE `tenant_id` = 'system' AND `code` = ? LIMIT 1",
            [code.into()],
        ))
        .await?;
    Ok(row.map(|row| i64::try_get_by_index(&row, 0)).transpose()?)
}

async fn next_permission_id<C>(db: &C) -> Result<i64, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    next_catalog_id(
        db,
        "SELECT COALESCE(MAX(`id`), 9999) + 1 FROM `sys_permission` WHERE `id` >= 10000",
    )
    .await
}

async fn menu_id<C>(db: &C, route_key: &str) -> Result<Option<i64>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT `id` FROM `sys_menu` \
             WHERE `tenant_id` = 'system' AND `route_key` = ? LIMIT 1",
            [route_key.into()],
        ))
        .await?;
    Ok(row.map(|row| i64::try_get_by_index(&row, 0)).transpose()?)
}

async fn next_menu_id<C>(db: &C) -> Result<i64, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    next_catalog_id(
        db,
        "SELECT COALESCE(MAX(`id`), 19999) + 1 FROM `sys_menu` WHERE `id` >= 20000",
    )
    .await
}

async fn next_catalog_id<C>(db: &C, sql: &'static str) -> Result<i64, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql))
        .await?
        .ok_or_else(|| DbErr::Custom("访问目录 ID 查询没有返回结果".into()))?;
    Ok(i64::try_get_by_index(&row, 0)?)
}

pub fn access_permission_codes() -> Result<Vec<String>, DbErr> {
    let mut values = Vec::new();
    let mut inside = false;
    for line in ACCESS_CATALOG.lines().map(str::trim) {
        if line == "permissions = [" && !inside {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line == "]" {
            break;
        }
        let value = line.trim_end_matches(',').trim_matches('"');
        if !is_permission_code(value) {
            return Err(DbErr::Custom("访问目录包含非法权限编码，拒绝初始化".into()));
        }
        values.push(value.to_owned());
    }
    if values.is_empty() {
        return Err(DbErr::Custom("访问目录没有权限定义".into()));
    }
    let mut merged = BTreeSet::new();
    for value in values {
        if !merged.insert(value.clone()) {
            return Err(DbErr::Custom(format!(
                "手写访问目录重复拥有权限码: {value}"
            )));
        }
    }
    for resource in generated_access_resources()? {
        for value in resource.permission_codes() {
            if !merged.insert(value.to_owned()) {
                return Err(DbErr::Custom(format!(
                    "生成权限码与其他资源或手写目录冲突: {value}"
                )));
            }
        }
    }
    Ok(merged.into_iter().collect())
}

pub fn access_permission_names() -> Result<BTreeMap<String, String>, DbErr> {
    let permissions = access_permission_codes()?;
    let mut names = BTreeMap::new();
    let mut inside = false;
    for line in ACCESS_CATALOG.lines().map(str::trim) {
        if line == "[permission_names]" {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line.starts_with('[') {
            break;
        }
        if line.is_empty() {
            continue;
        }
        let Some((code, name)) = catalog_map_entry(line) else {
            return Err(DbErr::Custom("访问目录包含非法权限名称，拒绝初始化".into()));
        };
        if !permissions.iter().any(|permission| permission == code)
            || name.trim() != name
            || name.is_empty()
            || name.chars().count() > 64
            || names.insert(code.to_owned(), name.to_owned()).is_some()
        {
            return Err(DbErr::Custom("访问目录包含非法权限名称，拒绝初始化".into()));
        }
    }
    Ok(names)
}

pub fn access_menus() -> Result<Vec<AccessMenu>, DbErr> {
    let mut menus = Vec::new();
    let mut current = None;
    for line in ACCESS_CATALOG.lines().map(str::trim) {
        if line == "[[menus]]" {
            if let Some(menu) = current.take() {
                menus.push(menu);
            }
            current = Some(AccessMenu {
                route_key: String::new(),
                name: String::new(),
                menu_type: String::new(),
                permission: None,
                parent_route_key: None,
                sort: 0,
            });
            continue;
        }
        if line.starts_with("[[") {
            if let Some(menu) = current.take() {
                menus.push(menu);
            }
            continue;
        }
        let Some(menu) = current.as_mut() else {
            continue;
        };
        if let Some(value) = catalog_string_value(line, "route_key") {
            menu.route_key = value.to_owned();
        } else if let Some(value) = catalog_string_value(line, "name") {
            menu.name = value.to_owned();
        } else if let Some(value) = catalog_string_value(line, "menu_type") {
            menu.menu_type = value.to_owned();
        } else if let Some(value) = catalog_string_value(line, "permission") {
            menu.permission = Some(value.to_owned());
        }
    }
    if let Some(menu) = current {
        menus.push(menu);
    }
    if menus.iter().any(|menu| {
        menu.route_key.is_empty()
            || menu.name.is_empty()
            || menu.name.chars().count() > 64
            || !matches!(menu.menu_type.as_str(), "M" | "C")
            || !menu
                .route_key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
    }) {
        return Err(DbErr::Custom("访问目录包含非法菜单定义".into()));
    }
    let permissions = access_permission_codes()?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut manual_route_keys = BTreeSet::new();
    for (index, menu) in menus.iter_mut().enumerate() {
        if !manual_route_keys.insert(menu.route_key.clone()) {
            return Err(DbErr::Custom(format!(
                "手写访问目录重复拥有菜单 route_key: {}",
                menu.route_key
            )));
        }
        menu.sort = i32::try_from(index)
            .map_err(|_| DbErr::Custom("访问目录菜单数量超出可表示范围".into()))?;
        menu.parent_route_key = (menu.menu_type == "C")
            .then(|| {
                menu.route_key
                    .split_once('.')
                    .map(|(parent, _)| parent.to_owned())
            })
            .flatten();
        if menu
            .permission
            .as_ref()
            .is_some_and(|code| !permissions.contains(code))
        {
            return Err(DbErr::Custom(format!(
                "菜单 {} 引用了未声明的权限码",
                menu.route_key
            )));
        }
    }

    let mut route_owners = manual_route_keys.clone();
    for resource in generated_access_resources()? {
        if !manual_route_keys.contains(&resource.menu.parent) {
            return Err(DbErr::Custom(format!(
                "生成菜单 {} 的父菜单 {} 不存在于手写访问目录",
                resource.menu.key, resource.menu.parent
            )));
        }
        if !route_owners.insert(resource.menu.key.clone()) {
            return Err(DbErr::Custom(format!(
                "生成菜单 route_key 与其他资源或手写目录冲突: {}",
                resource.menu.key
            )));
        }
        menus.push(AccessMenu {
            route_key: resource.menu.key,
            name: resource.menu.labels.zh_cn,
            menu_type: "C".to_owned(),
            permission: Some(resource.permissions.list),
            parent_route_key: Some(resource.menu.parent),
            sort: i32::try_from(resource.menu.order)
                .map_err(|_| DbErr::Custom("生成菜单 order 超出可表示范围".into()))?,
        });
    }
    menus.sort_by(|left, right| left.route_key.cmp(&right.route_key));
    Ok(menus)
}

fn catalog_string_value(line: &'static str, key: &str) -> Option<&'static str> {
    let value = line
        .strip_prefix(key)?
        .trim_start()
        .strip_prefix('=')?
        .trim();
    value.strip_prefix('"')?.strip_suffix('"')
}

fn catalog_map_entry(line: &'static str) -> Option<(&'static str, &'static str)> {
    let (key, value) = line.split_once('=')?;
    let key = key.trim().strip_prefix('"')?.strip_suffix('"')?;
    let value = value.trim().strip_prefix('"')?.strip_suffix('"')?;
    is_permission_code(key).then_some((key, value))
}

fn is_permission_code(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b":-*._".contains(&byte))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedAccessCatalog {
    version: u32,
    #[serde(default)]
    resources: Vec<GeneratedAccessResource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedAccessResource {
    name: String,
    module: String,
    capability: String,
    labels: GeneratedLabels,
    menu: GeneratedMenu,
    route: GeneratedRoute,
    permissions: GeneratedPermissions,
    #[serde(default)]
    extension_permissions: BTreeMap<String, String>,
}

impl GeneratedAccessResource {
    fn permission_codes(&self) -> BTreeSet<&str> {
        self.permissions
            .values()
            .into_iter()
            .chain(self.extension_permissions.values().map(String::as_str))
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedLabels {
    zh_cn: String,
    en: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedMenu {
    key: String,
    parent: String,
    order: u32,
    labels: GeneratedLabels,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedRoute {
    key: String,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedPermissions {
    create: String,
    read: String,
    list: String,
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

fn generated_access_resources() -> Result<Vec<GeneratedAccessResource>, DbErr> {
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
        validate_generated_code("生成资源能力码", &resource.capability, '.')?;
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
