//! 业务模块的装配入口。

use std::sync::Arc;

use axum::Router;
use ryframe_business_application::generated::{GeneratedPersistencePorts, GeneratedServices};
use ryframe_kernel::AppResult;
use ryframe_tenant_db::TenantDatabaseRouter;

/// 业务模块交给框架组合根的路由与契约。
pub struct BusinessModule {
    pub router: Router,
    pub openapi: ryframe_api::openapi::OpenApiDocument,
}

/// 装配业务持久化端口、用例、受保护路由和完整 OpenAPI。
pub fn build(
    state: ryframe_api::AppState,
    tenant_database: Arc<TenantDatabaseRouter>,
) -> AppResult<BusinessModule> {
    let mut ports = GeneratedPersistencePorts::default();
    ryframe_business_db::generated::tenant_ports(tenant_database, &mut ports);
    let services = GeneratedServices::try_new(ports)?;
    let router = ryframe_business_api::generated::generated_router(
        state.clone(),
        &services,
        state.settings.pagination,
    );
    Ok(BusinessModule {
        router,
        openapi: ryframe_business_api::combined_openapi(),
    })
}
