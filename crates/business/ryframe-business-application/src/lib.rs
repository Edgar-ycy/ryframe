//! 业务用例与持久化端口，由业务资源生成器扩展。

pub mod generated;

pub use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode, complete_transaction, next_id,
};
use ryframe_kernel::{ActorContext, AppResult, TenantId};

pub fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {
    TenantId::parse(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}
