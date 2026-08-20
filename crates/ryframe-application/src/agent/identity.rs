use crate::PersistenceFuture;

#[derive(Debug)]
pub struct AgentCredentialHint {
    pub id: i64,
    pub tenant_id: String,
    pub account_id: i64,
}

#[derive(Debug)]
pub struct AgentDelegationHint {
    pub id: i64,
    pub tenant_id: String,
    pub account_id: i64,
    pub user_id: i64,
    pub version: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentLimitHints {
    pub tenant_limit: i32,
    pub account_limit: Option<i32>,
}

pub trait AgentIdentityReadPort: Send + Sync {
    fn credential_hint<'a>(
        &'a self,
        key_id: &'a str,
    ) -> PersistenceFuture<'a, Option<AgentCredentialHint>>;

    fn delegation_hint<'a>(
        &'a self,
        token_mac_candidates: &'a [Vec<u8>],
    ) -> PersistenceFuture<'a, Option<AgentDelegationHint>>;

    fn limit_hints<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, AgentLimitHints>;
}
