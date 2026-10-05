use std::{borrow::Borrow, collections::BTreeSet};

#[cfg(feature = "api")]
use axum::Router;
#[cfg(feature = "api")]
use ryframe_api::{AppState, openapi::OpenApiDocument};
#[cfg(feature = "api")]
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::{AppError, AppResult};
#[cfg(feature = "api")]
use ryframe_tenant_db::TenantDatabaseRouter;

#[cfg(feature = "migration")]
use crate::BusinessMigration;
use crate::ResourceDescriptor;
#[cfg(any(feature = "api", feature = "migration"))]
use std::sync::Arc;

#[cfg(feature = "api")]
#[derive(Clone)]
pub struct BusinessRuntimeContext {
    pub state: AppState,
    pub control_database: ControlDatabaseCluster,
    pub tenant_database: Arc<TenantDatabaseRouter>,
}

#[cfg(feature = "api")]
pub type BusinessRouterFactory = fn(BusinessRuntimeContext) -> AppResult<Router>;
#[cfg(feature = "api")]
pub type BusinessOpenApiFactory = fn() -> OpenApiDocument;

#[derive(Clone)]
pub struct RyFrameBusinessModule {
    name: &'static str,
    dependencies: &'static [&'static str],
    resources: Vec<ResourceDescriptor>,
    #[cfg(feature = "api")]
    router: Option<BusinessRouterFactory>,
    #[cfg(feature = "api")]
    openapi: Option<BusinessOpenApiFactory>,
    #[cfg(feature = "migration")]
    migrations: Vec<Arc<dyn BusinessMigration>>,
}

impl RyFrameBusinessModule {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn dependencies(&self) -> &'static [&'static str] {
        self.dependencies
    }

    pub fn resources(&self) -> &[ResourceDescriptor] {
        &self.resources
    }

    #[cfg(feature = "api")]
    pub fn router(&self, context: BusinessRuntimeContext) -> AppResult<Router> {
        self.router
            .map_or_else(|| Ok(Router::new()), |build| build(context))
    }

    #[cfg(feature = "api")]
    pub fn openapi(&self) -> Option<OpenApiDocument> {
        self.openapi.map(|build| build())
    }

    #[cfg(feature = "migration")]
    pub fn migrations(&self) -> &[Arc<dyn BusinessMigration>] {
        &self.migrations
    }
}

pub struct BusinessModuleBuilder {
    module: RyFrameBusinessModule,
}

impl BusinessModuleBuilder {
    pub fn new(name: &'static str) -> Self {
        Self {
            module: RyFrameBusinessModule {
                name,
                dependencies: &[],
                resources: Vec::new(),
                #[cfg(feature = "api")]
                router: None,
                #[cfg(feature = "api")]
                openapi: None,
                #[cfg(feature = "migration")]
                migrations: Vec::new(),
            },
        }
    }

    pub fn dependencies(mut self, dependencies: &'static [&'static str]) -> Self {
        self.module.dependencies = dependencies;
        self
    }

    pub fn resources<I>(mut self, resources: I) -> Self
    where
        I: IntoIterator,
        I::Item: Borrow<ResourceDescriptor>,
    {
        self.module.resources = resources.into_iter().map(|item| *item.borrow()).collect();
        self
    }

    #[cfg(feature = "api")]
    pub fn routes(mut self, router: BusinessRouterFactory) -> Self {
        self.module.router = Some(router);
        self
    }

    #[cfg(feature = "api")]
    pub fn openapi(mut self, openapi: BusinessOpenApiFactory) -> Self {
        self.module.openapi = Some(openapi);
        self
    }

    #[cfg(feature = "migration")]
    pub fn migrations(mut self, migrations: Vec<Arc<dyn BusinessMigration>>) -> Self {
        self.module.migrations = migrations;
        self
    }

    pub fn build(self) -> RyFrameBusinessModule {
        #[cfg(feature = "migration")]
        {
            let mut module = self.module;
            module
                .migrations
                .sort_by(|left, right| left.id().cmp(right.id()));
            module
        }
        #[cfg(not(feature = "migration"))]
        {
            self.module
        }
    }
}

#[cfg(feature = "api")]
pub fn compose_openapi(
    mut document: OpenApiDocument,
    modules: &[RyFrameBusinessModule],
) -> AppResult<OpenApiDocument> {
    let mut operations = document_keys(&document)?;
    let mut resources = document
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("x-ryframe-crud-resources"))
        .and_then(|value| value.get("resources"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    for module in modules {
        let Some(fragment) = module.openapi() else {
            continue;
        };
        for operation in document_keys(&fragment)? {
            if !operations.insert(operation.clone()) {
                return Err(AppError::Config(format!(
                    "业务模块 {} 的 OpenAPI 路由冲突：{operation}",
                    module.name()
                )));
            }
        }
        resources.extend(
            fragment
                .extensions
                .as_ref()
                .and_then(|extensions| extensions.get("x-ryframe-crud-resources"))
                .and_then(|value| value.get("resources"))
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        document.merge(fragment);
    }
    resources.sort_by(|left, right| {
        left.get("name")
            .and_then(serde_json::Value::as_str)
            .cmp(&right.get("name").and_then(serde_json::Value::as_str))
    });
    document.extensions.get_or_insert_default().insert(
        "x-ryframe-crud-resources".into(),
        serde_json::json!({ "version": 1, "resources": resources }),
    );
    Ok(document)
}

pub fn validate_modules(modules: &[RyFrameBusinessModule]) -> AppResult<()> {
    let names = modules
        .iter()
        .map(RyFrameBusinessModule::name)
        .collect::<BTreeSet<_>>();
    if names.len() != modules.len() {
        return Err(AppError::Config("业务模块名称重复".into()));
    }
    let mut resource_names = BTreeSet::new();
    let mut table_names = BTreeSet::new();
    let mut routes = BTreeSet::new();
    let mut menu_keys = BTreeSet::new();
    for module in modules {
        validate_module(module)?;
        for resource in module.resources() {
            if !resource_names.insert(resource.name) {
                return Err(AppError::Config(format!(
                    "业务资源名称冲突：{}",
                    resource.name
                )));
            }
            if !table_names.insert(resource.table) {
                return Err(AppError::Config(format!(
                    "业务表名冲突：{}",
                    resource.table
                )));
            }
            if let Some(route) = resource.route
                && !routes.insert(route)
            {
                return Err(AppError::Config(format!("业务路由冲突：{route}")));
            }
            let menu_key = format!("{}.{}", module.name(), resource.name);
            if !menu_keys.insert(menu_key.clone()) {
                return Err(AppError::Config(format!("业务菜单键冲突：{menu_key}")));
            }
        }
        for dependency in module.dependencies() {
            if !names.contains(dependency) {
                return Err(AppError::Config(format!(
                    "业务模块 {} 依赖未注册模块 {dependency}",
                    module.name()
                )));
            }
        }
    }
    reject_cycles(modules)
}

pub fn sort_modules(modules: &mut [RyFrameBusinessModule]) -> AppResult<()> {
    validate_modules(modules)?;
    let mut depth = std::collections::BTreeMap::new();
    fn module_depth(
        name: &'static str,
        modules: &[RyFrameBusinessModule],
        cache: &mut std::collections::BTreeMap<&'static str, usize>,
    ) -> usize {
        if let Some(depth) = cache.get(name) {
            return *depth;
        }
        let module = modules
            .iter()
            .find(|module| module.name() == name)
            .expect("依赖已经通过 validate_modules 校验");
        let value = module
            .dependencies()
            .iter()
            .map(|dependency| module_depth(dependency, modules, cache) + 1)
            .max()
            .unwrap_or(0);
        cache.insert(name, value);
        value
    }
    for module in modules.iter() {
        module_depth(module.name(), modules, &mut depth);
    }
    modules.sort_by(|left, right| {
        depth[left.name()]
            .cmp(&depth[right.name()])
            .then(left.name().cmp(right.name()))
    });
    Ok(())
}

fn validate_module(module: &RyFrameBusinessModule) -> AppResult<()> {
    if !safe_key(module.name) {
        return Err(AppError::Config(format!(
            "业务模块名称无效：{}",
            module.name
        )));
    }
    let mut resources = BTreeSet::new();
    let mut tables = BTreeSet::new();
    for resource in &module.resources {
        if !resources.insert(resource.name) || !tables.insert(resource.table) {
            return Err(AppError::Config(format!(
                "业务模块 {} 存在重复资源或表名",
                module.name
            )));
        }
    }
    #[cfg(feature = "migration")]
    if module
        .migrations
        .iter()
        .any(|migration| migration.module() != module.name)
    {
        return Err(AppError::Config(format!(
            "业务模块 {} 包含归属不一致的迁移",
            module.name
        )));
    }
    Ok(())
}

fn reject_cycles(modules: &[RyFrameBusinessModule]) -> AppResult<()> {
    fn visit(
        name: &'static str,
        modules: &[RyFrameBusinessModule],
        visiting: &mut BTreeSet<&'static str>,
        visited: &mut BTreeSet<&'static str>,
    ) -> AppResult<()> {
        if visited.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name) {
            return Err(AppError::Config(format!("业务模块依赖存在循环：{name}")));
        }
        let module = modules
            .iter()
            .find(|module| module.name() == name)
            .ok_or_else(|| AppError::Config(format!("业务模块依赖未注册：{name}")))?;
        for dependency in module.dependencies() {
            visit(dependency, modules, visiting, visited)?;
        }
        visiting.remove(name);
        visited.insert(name);
        Ok(())
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for module in modules {
        visit(module.name(), modules, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn safe_key(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

#[cfg(feature = "api")]
fn document_keys(document: &OpenApiDocument) -> AppResult<BTreeSet<String>> {
    let value = serde_json::to_value(document)
        .map_err(|error| AppError::Internal(format!("OpenAPI 序列化失败：{error}")))?;
    let mut keys = BTreeSet::new();
    if let Some(paths) = value.get("paths").and_then(serde_json::Value::as_object) {
        for (path, item) in paths {
            if let Some(methods) = item.as_object() {
                for method in methods.keys().filter(|method| {
                    matches!(
                        method.as_str(),
                        "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "trace"
                    )
                }) {
                    keys.insert(format!("route:{} {}", method.to_ascii_uppercase(), path));
                    if let Some(operation_id) = item
                        .get(method)
                        .and_then(|operation| operation.get("operationId"))
                        .and_then(serde_json::Value::as_str)
                    {
                        keys.insert(format!("operation:{operation_id}"));
                    }
                }
            }
        }
    }
    if let Some(schemas) = value
        .get("components")
        .and_then(|components| components.get("schemas"))
        .and_then(serde_json::Value::as_object)
    {
        keys.extend(schemas.keys().map(|name| format!("schema:{name}")));
    }
    Ok(keys)
}
