//! 服务账号查询、授权、审计和写入端口。

mod audit;
mod authorization;
mod read;
mod write;

pub use audit::{ServiceAccessAuditRecord, ServiceAccountAuditReadPort};
pub use authorization::{
    ServiceAccountAuthorizationReadPort, ServiceAccountPermissionSnapshot,
    ServiceDelegationTargetRecord, ServiceDelegationTargetSet,
};
pub use read::{
    ServiceAccountDetailRecord, ServiceAccountReadPort, ServiceAccountRecord,
    ServiceCredentialRecord, ServiceDelegationRecord,
};
pub use write::{
    ServiceAccountUserRecord, ServiceAccountWritePort, ServiceAccountWriteTransaction,
    ServiceCredentialWriteRecord, ServiceDelegationIdentity, ServiceDelegationWriteRecord,
};
