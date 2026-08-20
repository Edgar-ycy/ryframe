use std::sync::Arc;

use ryframe_db::{
    ControlDatabaseCluster, ServiceAccountLock, ServiceAccountRepository,
    ServiceCredentialRepository, ServiceDelegationRepository,
};
use ryframe_kernel::AppError;
use sea_orm::TransactionTrait;

use crate::{
    PersistenceFuture,
    agent::{AgentCredentialHint, AgentDelegationHint, AgentIdentityReadPort, AgentLimitHints},
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn AgentIdentityReadPort> {
    Arc::new(LegacyAgentIdentityRead { database })
}

struct LegacyAgentIdentityRead {
    database: ControlDatabaseCluster,
}

impl AgentIdentityReadPort for LegacyAgentIdentityRead {
    fn credential_hint<'a>(
        &'a self,
        key_id: &'a str,
    ) -> PersistenceFuture<'a, Option<AgentCredentialHint>> {
        Box::pin(async move {
            ServiceCredentialRepository
                .find_hint_by_key_id(self.database.write(), key_id)
                .await
                .map(|credential| {
                    credential.map(|credential| AgentCredentialHint {
                        id: credential.id,
                        tenant_id: credential.tenant_id,
                        account_id: credential.account_id,
                    })
                })
        })
    }

    fn delegation_hint<'a>(
        &'a self,
        token_mac_candidates: &'a [Vec<u8>],
    ) -> PersistenceFuture<'a, Option<AgentDelegationHint>> {
        Box::pin(async move {
            ServiceDelegationRepository
                .find_by_mac_candidates(self.database.write(), token_mac_candidates)
                .await
                .map(|delegation| {
                    delegation.map(|delegation| AgentDelegationHint {
                        id: delegation.id,
                        tenant_id: delegation.tenant_id,
                        account_id: delegation.account_id,
                        user_id: delegation.user_id,
                        version: delegation.version,
                    })
                })
        })
    }

    fn limit_hints<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, AgentLimitHints> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            let result = async {
                let tenant = ServiceAccountRepository
                    .lock_tenant_in_txn(&transaction, tenant_id, ServiceAccountLock::Share)
                    .await?;
                let account = ServiceAccountRepository
                    .find_by_id_in_txn(
                        &transaction,
                        &tenant.tenant_id,
                        account_id,
                        ServiceAccountLock::Share,
                    )
                    .await?;
                Ok(AgentLimitHints {
                    tenant_limit: tenant.max_requests_per_min,
                    account_limit: account.map(|account| account.max_requests_per_minute),
                })
            }
            .await;
            let _ = transaction.rollback().await;
            result
        })
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
