mod auth_service;
pub mod system;

pub use auth_service::{AuthService, LoginResult, UserInfo};

use ryframe_common::{ActorContext, AppResult};

pub(crate) fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {
    ryframe_core::validate_explicit_tenant(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}
