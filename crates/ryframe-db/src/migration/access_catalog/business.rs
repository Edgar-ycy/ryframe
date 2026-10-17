//! 业务模型生成的访问事实；数据库初始化与 OpenAPI 装配共用同一解析结果。

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

use sea_orm::DbErr;

use super::{
    AccessMenu, builtin_access_menus, builtin_access_permission_codes,
    generated::{GeneratedAccessResource, generated_access_resources, parse_access_resources},
};

mod verify;
pub use verify::verify_business_access_catalog;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BusinessResourceAccess {
    pub module: String,
    pub resource: String,
    pub route_key: String,
    pub route_path: String,
    pub parent: String,
    pub permissions: [String; 5],
    pub extension_permissions: BTreeMap<String, String>,
    pub system_tenant_only: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BusinessAccessCatalog {
    resources: BTreeMap<String, BusinessResourceAccess>,
    menus: Vec<AccessMenu>,
    permissions: BTreeMap<String, String>,
    system_permissions: BTreeSet<String>,
    system_routes: BTreeSet<String>,
}

impl BusinessAccessCatalog {
    pub fn resources(&self) -> impl Iterator<Item = &BusinessResourceAccess> {
        self.resources.values()
    }
    pub fn resource(&self, module: &str, resource: &str) -> Option<&BusinessResourceAccess> {
        self.resources.get(&format!("{module}::{resource}"))
    }
    pub fn menus(&self) -> &[AccessMenu] {
        &self.menus
    }
    pub fn permission_names(&self) -> &BTreeMap<String, String> {
        &self.permissions
    }
    pub fn system_permissions(&self) -> &BTreeSet<String> {
        &self.system_permissions
    }
    pub fn system_routes(&self) -> &BTreeSet<String> {
        &self.system_routes
    }
}

static REGISTERED: OnceLock<BusinessAccessCatalog> = OnceLock::new();

pub(super) fn registered() -> Option<&'static BusinessAccessCatalog> {
    REGISTERED.get()
}

/// 整批编译，不读取数据库或更改进程目录；所有来源都使用生成访问目录的同一格式。
pub fn compile_business_access_catalog<'a>(
    sources: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<BusinessAccessCatalog, DbErr> {
    let mut compiler = Compiler::new()?;
    let mut modules = BTreeSet::new();
    for (module, source) in sources {
        validate_module(module)?;
        if !modules.insert(module) {
            return Err(invalid(format!("业务访问目录模块重复：{module}")));
        }
        for resource in
            parse_access_resources(source, &format!("业务模块 {module} 的生成访问目录"))?
        {
            compiler.insert(module, resource)?;
        }
    }
    compiler.finish()
}

/// 组合根在启动前一次登记；相同事实可重复核验，不能在运行期间更换授权边界。
pub fn register_business_access_catalog<'a>(
    sources: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<&'static BusinessAccessCatalog, DbErr> {
    let candidate = compile_business_access_catalog(sources)?;
    if let Some(current) = REGISTERED.get() {
        return identical(current, &candidate);
    }
    match REGISTERED.set(candidate) {
        Ok(()) => REGISTERED
            .get()
            .ok_or_else(|| invalid("业务访问目录安装后不可用")),
        Err(candidate) => identical(
            REGISTERED
                .get()
                .ok_or_else(|| invalid("业务访问目录并发安装后不可用"))?,
            &candidate,
        ),
    }
}

fn identical<'a>(
    current: &'a BusinessAccessCatalog,
    candidate: &BusinessAccessCatalog,
) -> Result<&'a BusinessAccessCatalog, DbErr> {
    if current == candidate {
        Ok(current)
    } else {
        Err(invalid(
            "业务访问目录已登记，不能在运行期间替换；重新构建并重启进程",
        ))
    }
}

struct Compiler {
    catalog: BusinessAccessCatalog,
    names: BTreeSet<String>,
    codes: BTreeSet<String>,
    routes: BTreeSet<String>,
    parents: BTreeSet<String>,
    groups: BTreeMap<String, bool>,
}

impl Compiler {
    fn new() -> Result<Self, DbErr> {
        let menus = builtin_access_menus()?;
        Ok(Self {
            catalog: BusinessAccessCatalog::default(),
            names: generated_access_resources()?
                .into_iter()
                .map(|resource| resource.name)
                .collect(),
            codes: builtin_access_permission_codes()?.into_iter().collect(),
            routes: menus.iter().map(|menu| menu.route_key.clone()).collect(),
            parents: menus
                .iter()
                .filter(|menu| menu.menu_type == "M")
                .map(|menu| menu.route_key.clone())
                .collect(),
            groups: BTreeMap::new(),
        })
    }

    fn insert(&mut self, module: &str, resource: GeneratedAccessResource) -> Result<(), DbErr> {
        let namespace = module.replace('_', "-");
        if resource.capability.is_some() {
            return Err(invalid(
                "标准业务 ResourceModel 当前不声明可选能力；不能手工为生成访问目录添加 capability",
            ));
        }
        validate_resource_key(&namespace, &resource)?;
        if resource.module != "business" || !self.names.insert(resource.name.clone()) {
            return Err(invalid(format!(
                "业务资源名称与其他目录冲突或来源不是 business：{}",
                resource.name
            )));
        }
        if !self.routes.insert(resource.menu.key.clone()) {
            return Err(invalid(format!(
                "业务菜单路由与其他目录冲突：{}",
                resource.menu.key
            )));
        }
        for code in resource.permission_codes() {
            validate_permission(&namespace, code)?;
            if !self.codes.insert(code.to_owned()) {
                return Err(invalid(format!("业务权限与其他资源或目录冲突：{code}")));
            }
            let action = permission_action(code, &resource);
            let name = format!("{}：{action}", resource.menu.labels.zh_cn);
            if name.chars().count() > 64 {
                return Err(invalid(
                    "业务权限名称超过 64 字符；为资源模型设置较短的 title 后重新生成",
                ));
            }
            self.catalog.permissions.insert(code.into(), name);
            if resource.system_tenant_only {
                self.catalog.system_permissions.insert(code.into());
            }
        }
        if resource.menu.parent == namespace && !self.parents.contains(&namespace) {
            self.groups
                .entry(namespace)
                .and_modify(|global| *global &= resource.system_tenant_only)
                .or_insert(resource.system_tenant_only);
        }
        if resource.system_tenant_only {
            self.catalog.system_routes.insert(resource.menu.key.clone());
        }
        let binding = BusinessResourceAccess {
            module: module.into(),
            resource: resource.name.clone(),
            route_key: resource.menu.key.clone(),
            route_path: resource.route.path,
            parent: resource.menu.parent.clone(),
            permissions: resource.permissions.values().map(str::to_owned),
            system_tenant_only: resource.system_tenant_only,
            extension_permissions: resource.extension_permissions,
        };
        self.catalog
            .resources
            .insert(format!("{module}::{}", resource.name), binding);
        self.catalog.menus.push(AccessMenu {
            route_key: resource.menu.key,
            name: resource.menu.labels.zh_cn,
            menu_type: "C".into(),
            permission: Some(resource.permissions.list),
            icon: resource.menu.icon,
            parent_route_key: Some(resource.menu.parent),
            sort: i32::try_from(resource.menu.order)
                .map_err(|_| invalid("业务菜单 order 超出可表示范围"))?,
            sort_declared: true,
        });
        Ok(())
    }

    fn finish(mut self) -> Result<BusinessAccessCatalog, DbErr> {
        for (key, global) in self.groups {
            if !self.routes.insert(key.clone()) {
                return Err(invalid(format!("业务目录与页面路由冲突：{key}")));
            }
            self.parents.insert(key.clone());
            if global {
                self.catalog.system_routes.insert(key.clone());
            }
            self.catalog.menus.push(AccessMenu {
                route_key: key.clone(),
                name: key,
                menu_type: "M".into(),
                permission: None,
                icon: Some("FolderOpened".into()),
                parent_route_key: None,
                sort: 100,
                sort_declared: true,
            });
        }
        for menu in &self.catalog.menus {
            if let Some(parent) = menu.parent_route_key()
                && !self.parents.contains(parent)
            {
                return Err(invalid(format!(
                    "业务菜单 {} 的父目录 {parent} 不存在或不是目录；修改 menu_parent 后重新生成",
                    menu.route_key
                )));
            }
        }
        self.catalog.menus.sort_by(|left, right| {
            (&left.menu_type, &left.route_key).cmp(&(&right.menu_type, &right.route_key))
        });
        // 目录先于页面，确保数据库种子可以按同一顺序解析父节点。
        self.catalog.menus.sort_by_key(|menu| menu.menu_type != "M");
        Ok(self.catalog)
    }
}

fn validate_module(module: &str) -> Result<(), DbErr> {
    if !module
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase)
        || module.len() > 64
        || module.split(['-', '_']).any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
    {
        return Err(invalid("业务访问目录模块名必须为不超过 64 字节的小写标识"));
    }
    Ok(())
}

fn validate_resource_key(module: &str, resource: &GeneratedAccessResource) -> Result<(), DbErr> {
    if !resource
        .name
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase)
        || resource.name.split('_').any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
        || resource.menu.key != format!("{module}.{}", resource.name.replace('_', "-"))
    {
        return Err(invalid(
            "业务菜单路由必须与模块和资源名称一致；运行生成器更新目录",
        ));
    }
    Ok(())
}

fn validate_permission(module: &str, code: &str) -> Result<(), DbErr> {
    let parts = code.split(':').collect::<Vec<_>>();
    if parts.len() != 3
        || parts[0] != module
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err(invalid(format!(
            "业务权限必须使用模块 {module} 的三个小写分段：{code}"
        )));
    }
    Ok(())
}

fn permission_action(code: &str, resource: &GeneratedAccessResource) -> &'static str {
    let [create, read, list, update, delete] = resource.permissions.values();
    if code == create {
        "新增"
    } else if code == read || code == list {
        "查询"
    } else if code == update {
        "编辑"
    } else if code == delete {
        "删除"
    } else {
        "扩展"
    }
}

fn invalid(message: impl Into<String>) -> DbErr {
    DbErr::Custom(message.into())
}
