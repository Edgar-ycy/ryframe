mod provisioning;
mod registry;
mod runtime;
mod template;
mod usage;

pub use provisioning::{TenantProvisioningPlacement, TenantProvisioningPort};
pub use registry::{
    ProvisionTenantRecord, TENANT_STATUS_DISABLED, TENANT_STATUS_ENABLED,
    TENANT_STATUS_PROVISIONING, TENANT_STATUS_PROVISIONING_FAILED, TenantAdminRecord,
    TenantPersistencePort, TenantProductAssignmentRecord, TenantProvisionRequestRecord,
    TenantRecord, TenantTransaction,
};
pub use runtime::{TenantBusinessDataState, TenantRuntimeReadPort, TenantRuntimeSnapshot};
pub use template::{
    TenantAuthorizationTemplate, TenantBaseCatalogTemplate, TenantConfigTemplate,
    TenantDepartmentTemplate, TenantDictionaryDataTemplate, TenantDictionaryTypeTemplate,
    TenantMenuTemplate, TenantPermissionTemplate, TenantPostTemplate, TenantProvisioningIdentity,
    TenantProvisioningTemplate,
};
pub use usage::{
    TenantCapacityRecord, TenantUsageAggregateRecord, TenantUsageFilter, TenantUsagePersistencePort,
};
