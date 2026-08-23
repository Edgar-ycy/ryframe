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

impl AgentLimitHints {
    /// 将可选账号限额与配置默认值收敛为限流器使用的有界值。
    pub fn effective_limits(self, default_account_limit: u32) -> (i32, i32) {
        (
            self.tenant_limit,
            self.account_limit
                .unwrap_or_else(|| i32::try_from(default_account_limit).unwrap_or(i32::MAX)),
        )
    }
}

#[async_trait::async_trait]
pub trait AgentIdentityReadPort: Send + Sync {
    async fn credential_hint<'a>(
        &'a self,
        key_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<AgentCredentialHint>>;

    async fn delegation_hint<'a>(
        &'a self,
        token_mac_candidates: &'a [Vec<u8>],
    ) -> ryframe_kernel::AppResult<Option<AgentDelegationHint>>;

    async fn limit_hints<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> ryframe_kernel::AppResult<AgentLimitHints>;
}
