use std::sync::Arc;

use ryframe_application::{
    JobHandler,
    system::{
        TenantConfigApplyJobHandler, TenantConfigExportJobHandler, TenantConfigPreviewJobHandler,
        TenantConfigRollbackJobHandler, TenantDataMigrationJobHandler,
    },
};

use super::super::JobWorkerDependencies;

pub(super) fn handlers(dependencies: &JobWorkerDependencies) -> Vec<Arc<dyn JobHandler>> {
    let config = &dependencies.tenant_config_transfer;
    vec![
        Arc::new(TenantConfigExportJobHandler::new(config.clone())),
        Arc::new(TenantConfigPreviewJobHandler::new(config.clone())),
        Arc::new(TenantConfigApplyJobHandler::new(config.clone())),
        Arc::new(TenantConfigRollbackJobHandler::new(config.clone())),
        Arc::new(TenantDataMigrationJobHandler::new(
            dependencies.tenant_data_migration.clone(),
        )),
    ]
}
