//! RyFrame 业务模块开发接口。
//!
//! 业务 crate 依赖本 crate，主程序再显式依赖并注册业务 crate，避免业务代码与
//! RyFrame 组合根形成 Cargo 循环依赖。

#[cfg(feature = "migration")]
mod migration;
#[cfg(feature = "runtime")]
mod module;
mod resource;

#[cfg(feature = "persistence")]
pub use async_trait::async_trait;
#[cfg(feature = "migration")]
pub use migration::{
    BusinessMigration, BusinessMigrationScope, MigrationState, SeaOrmBusinessMigration,
};
#[cfg(feature = "runtime")]
pub use module::{BusinessModuleBuilder, RyFrameBusinessModule, sort_modules, validate_modules};
#[cfg(feature = "api")]
pub use module::{BusinessRuntimeContext, compose_openapi};
pub use resource::{ResourceDatabase, ResourceDescriptor, ResourceFieldDescriptor, ResourceModel};
#[cfg(feature = "api")]
pub use ryframe_api::{AppState, openapi::OpenApiDocument};
#[cfg(feature = "runtime")]
pub use ryframe_application::PersistenceTransaction;
#[cfg(feature = "runtime")]
pub use ryframe_application::{TransactionAuditMode, complete_transaction, next_id};
#[cfg(feature = "runtime")]
pub use ryframe_kernel::{
    ActorContext, AppError, AppResult, PageResult, PaginationPolicy, ValidatedPageQuery,
};
pub use ryframe_macro::{ResourceModel, capability, delete, get, patch, perm, post, put, route};
#[cfg(feature = "persistence")]
pub use sea_orm::{DatabaseConnection, TransactionTrait};

#[cfg(feature = "runtime")]
pub fn validated_tenant_id(actor: &ryframe_kernel::ActorContext) -> AppResult<&str> {
    ryframe_kernel::TenantId::parse(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}

pub use chrono;
pub use serde;
#[cfg(feature = "api")]
pub use {axum, serde_json, utoipa, validator};

/// 生成的业务持久化代码使用的依赖统一从此模块取得，业务模型不需要暴露 ORM 类型。
#[cfg(feature = "persistence")]
pub mod persistence {
    pub use ryframe_db::{ControlDatabaseCluster, DbResultExt, ReadConsistency, pagination};
    pub use ryframe_tenant_db::{TenantDataError, TenantDatabaseRouter};
    pub use sea_orm;
    #[cfg(feature = "migration")]
    pub use sea_orm_migration;
}

#[cfg(feature = "api")]
pub mod api {
    pub use ryframe_api::auth_middleware::perm_route;
    pub use ryframe_api::http;
    pub use ryframe_api::{RequestPrincipal, parse_id};
}
