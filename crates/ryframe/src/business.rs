//! 使用者业务 crate 的显式注册表。
//!
//! 新建业务 crate 后，在 `crates/ryframe/Cargo.toml` 添加依赖，并在
//! `business_modules` 返回值中加入 `your_business::module()`。API、Worker 与迁移
//! 二进制共用此处，禁止各进程维护不同的模块列表。

use ryframe_kernel::AppResult;
use ryframe_sdk::{RyFrameBusinessModule, sort_modules};

/// 当前应用显式启用的业务模块。RyFrame 官方仓库默认不内置任何业务模块。
pub fn business_modules() -> Vec<RyFrameBusinessModule> {
    vec![]
}

pub fn registered_business_modules() -> AppResult<Vec<RyFrameBusinessModule>> {
    let mut modules = business_modules();
    sort_modules(&mut modules)?;
    let access = ryframe_db::migration::compile_business_access_catalog(
        modules
            .iter()
            .map(|module| (module.name(), module.access_catalog())),
    )
    .map_err(|error| ryframe_kernel::AppError::Config(error.to_string()))?;
    validate_business_access(&modules, &access)?;
    ryframe_db::migration::register_business_tables(modules.iter().flat_map(|module| {
        module
            .control_tables()
            .iter()
            .map(|table| (table.table, table.schema_canonical))
    }))
    .map_err(|error| ryframe_kernel::AppError::Config(error.to_string()))?;
    ryframe_tenant_db::migration::register_business_catalog(
        modules
            .iter()
            .flat_map(|module| module.tenant_data_tables().iter().copied()),
    )
    .map_err(ryframe_kernel::AppError::Config)?;
    let access = ryframe_db::migration::register_business_access_catalog(
        modules
            .iter()
            .map(|module| (module.name(), module.access_catalog())),
    )
    .map_err(|error| ryframe_kernel::AppError::Config(error.to_string()))?;
    ryframe_application::system::platform::register_business_access_scope(
        access.permission_names().keys().cloned(),
        access
            .resources()
            .filter(|resource| !resource.system_tenant_only)
            .flat_map(|resource| resource.permissions.iter().cloned()),
        access.system_permissions().iter().cloned(),
        access.system_routes().iter().cloned(),
    )?;
    Ok(modules)
}

fn validate_business_access(
    modules: &[RyFrameBusinessModule],
    access: &ryframe_db::migration::BusinessAccessCatalog,
) -> AppResult<()> {
    for module in modules {
        let rows = access
            .resources()
            .filter(|resource| resource.module == module.name())
            .count();
        if rows != module.resources().len() {
            return Err(ryframe_kernel::AppError::Config(format!(
                "业务模块 {} 的资源与访问目录数量不一致；运行 cargo generate -- resource --write",
                module.name()
            )));
        }
        for resource in module.resources() {
            let binding = access
                .resource(module.name(), resource.name)
                .ok_or_else(|| {
                    ryframe_kernel::AppError::Config(format!(
                        "业务资源 {} 缺少生成访问目录；运行 cargo generate -- resource --write",
                        resource.name
                    ))
                })?;
            if binding.system_tenant_only
                != (resource.database == ryframe_sdk::ResourceDatabase::Control)
            {
                return Err(ryframe_kernel::AppError::Config(format!(
                    "业务资源 {} 的数据库范围与访问边界不一致",
                    resource.name
                )));
            }
            if !binding.system_tenant_only
                && binding
                    .permissions
                    .iter()
                    .any(|code| ryframe_application::system::platform::is_platform_permission(code))
            {
                return Err(ryframe_kernel::AppError::Config(format!(
                    "租户业务资源 {} 使用了系统租户保留的权限命名空间",
                    resource.name
                )));
            }
        }
    }
    Ok(())
}
