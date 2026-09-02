//! 从访问控制事实源构造进程共享的配置迁移目标目录。

use ryframe_application::system::platform::TenantConfigTargetCatalog;
use ryframe_kernel::{AppError, AppResult};

pub fn tenant_config_target_catalog() -> AppResult<TenantConfigTargetCatalog> {
    let routes = ryframe_db::migration::access_menus()
        .map_err(catalog_error)?
        .into_iter()
        .map(|menu| (menu.route_key, menu.menu_type));
    let permissions = ryframe_db::migration::access_permission_codes().map_err(catalog_error)?;
    TenantConfigTargetCatalog::new(routes, permissions)
}

fn catalog_error(error: sea_orm::DbErr) -> AppError {
    AppError::Config(format!("访问控制目录无效: {error}"))
}
