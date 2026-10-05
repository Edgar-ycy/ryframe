//! 使用者业务 crate 的显式注册表。
//!
//! 新建业务 crate 后，在 `crates/ryframe/Cargo.toml` 添加依赖，并在
//! `business_modules` 返回值中加入 `your_business::module()`。API、Worker 与迁移
//! 二进制共用此处，禁止各进程维护不同的模块列表。

use ryframe_kernel::AppResult;
use ryframe_sdk::{ResourceDatabase, RyFrameBusinessModule, sort_modules};

/// 当前应用显式启用的业务模块。RyFrame 官方仓库默认不内置任何业务模块。
pub fn business_modules() -> Vec<RyFrameBusinessModule> {
    vec![]
}

pub fn registered_business_modules() -> AppResult<Vec<RyFrameBusinessModule>> {
    let mut modules = business_modules();
    sort_modules(&mut modules)?;
    ryframe_db::migration::register_business_tables(modules.iter().flat_map(|module| {
        module
            .resources()
            .iter()
            .filter(|resource| resource.database == ResourceDatabase::Control)
            .map(|resource| resource.table)
    }))
    .map_err(|error| ryframe_kernel::AppError::Config(error.to_string()))?;
    ryframe_tenant_db::migration::register_business_tables(modules.iter().flat_map(|module| {
        module
            .resources()
            .iter()
            .filter(|resource| resource.database == ResourceDatabase::Tenant)
            .map(|resource| resource.table)
    }))
    .map_err(|error| ryframe_kernel::AppError::Config(error.to_string()))?;
    Ok(modules)
}
