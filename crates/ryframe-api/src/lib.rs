use ryframe_application::system::TenantConfigTargetCatalog;
use ryframe_config::AppConfig;
use ryframe_kernel::{AppError, AppResult};

#[doc(hidden)]
pub mod __macro_support {
    pub use crate::auth_middleware::perm_route;
}

pub mod auth_middleware;
mod captcha;
pub mod dto;
mod handler_utils;
pub mod handlers;
pub mod http;
#[macro_use]
pub mod macros;
pub mod message_presenter;
pub mod message_socket;
pub mod middleware;
pub mod monitor;
pub mod openapi;
pub mod oper_log_middleware;
pub mod permission_catalog;
mod principal;
pub mod probes;
pub mod request_locale;
pub mod router;
pub mod runtime;
pub mod state;
pub mod versioning;

pub use handlers::common_handler::{download_router, upload_router};
pub use principal::RequestPrincipal;
pub use probes::{livez, readyz};
pub use request_locale::RequestLocale;
pub use router::{api_router, auth_router};
pub use state::{AppServices, AppState};
pub use versioning::{ApiVersion, VersionedRouter};

pub const RUNTIME_SWAGGER_UI_AVAILABLE: bool = cfg!(feature = "runtime-swagger-ui");

/// 返回当前 API 二进制编译时注册的租户配置迁移目标目录。
pub fn tenant_config_target_catalog() -> AppResult<TenantConfigTargetCatalog> {
    TenantConfigTargetCatalog::new(
        permission_catalog::menu_routes()
            .iter()
            .map(|menu| (menu.route_key.to_owned(), menu.menu_type.to_owned())),
        permission_catalog::permission_codes()
            .iter()
            .map(|code| (*code).to_owned()),
    )
}

/// 校验配置要求的运行时组件已经随当前二进制编译。
pub fn validate_runtime_features(config: &AppConfig) -> AppResult<()> {
    if config.api_docs.enabled && !RUNTIME_SWAGGER_UI_AVAILABLE {
        return Err(AppError::Config(
            "api_docs.enabled = true 时必须启用 runtime-swagger-ui feature".into(),
        ));
    }
    Ok(())
}
