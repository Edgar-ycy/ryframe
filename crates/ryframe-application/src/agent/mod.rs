mod audit;
mod identity;
mod limiter;
mod registry;
mod scope;
mod service;
mod storage;
mod types;

pub use audit::{AgentAccessAuditDraft, AgentAccessAuditRecord, AgentAuditWritePort};
pub use identity::{
    AgentCredentialHint, AgentDelegationHint, AgentIdentityReadPort, AgentLimitHints,
};
pub use limiter::{
    AgentConcurrencyLease, AgentLeaseReleaseFuture, AgentLimitFuture, AgentLimitInput, AgentLimiter,
};
pub use registry::{AgentCapability, AgentCapabilityDescriptor, service_capability_descriptors};
pub use scope::{
    AgentAuthorizationSnapshot, AgentDepartmentSnapshot, AgentPermissionSnapshot,
    AgentRoleDepartmentSnapshot, AgentRolePermissionSnapshot, AgentRoleSnapshot, AgentRowScope,
    AgentUserSnapshot,
};
pub use service::{AgentService, AgentServiceDependencies};
pub use storage::{
    AgentAccountRecord, AgentCredentialRecord, AgentDelegationRecord, AgentDepartmentRecord,
    AgentDictionaryItemRecord, AgentDictionaryPageRecord, AgentPersistencePort,
    AgentPersistenceTransaction, AgentPostRecord, AgentQueryPage, AgentTenantRecord,
    AgentUserRecord,
};
pub use types::{
    AgentAccessMode, AgentCapabilityVo, AgentDepartmentVo, AgentDictionaryItemVo,
    AgentDictionaryVo, AgentPage, AgentPostVo, AgentPrincipal, AgentRequest, AgentSuccess,
    AgentUserVo,
};
