use std::{collections::BTreeSet, sync::Arc};

use ryframe_db::{
    AgentQueryRepository, ControlDatabaseCluster, DataRetentionRepository, ProductRepository,
    ServiceAccessAuditRepository, ServiceAccountLock, ServiceAccountRepository,
    ServiceAuthorizationRepository, ServiceCredentialRepository, ServiceDelegationRepository,
    entities::{
        dept, dict_data, post, service_account, service_credential, service_delegation, tenant,
        user,
    },
};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use crate::{
    PersistenceFuture,
    agent::{
        AgentAccessAuditRecord, AgentAccountRecord, AgentAuthorizationSnapshot,
        AgentCredentialRecord, AgentDelegationRecord, AgentDepartmentRecord,
        AgentDictionaryItemRecord, AgentDictionaryPageRecord, AgentPersistencePort,
        AgentPersistenceTransaction, AgentPostRecord, AgentQueryPage, AgentRowScope,
        AgentTenantRecord, AgentUserRecord,
    },
    system::ProductService,
};

pub fn port(
    database: ControlDatabaseCluster,
    product: Arc<ProductService>,
) -> Arc<dyn AgentPersistencePort> {
    Arc::new(LegacyAgentPersistence { database, product })
}

struct LegacyAgentPersistence {
    database: ControlDatabaseCluster,
    product: Arc<ProductService>,
}

struct LegacyAgentTransaction {
    transaction: DatabaseTransaction,
    product: Arc<ProductService>,
}

impl AgentPersistencePort for LegacyAgentPersistence {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn AgentPersistenceTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(LegacyAgentTransaction {
                transaction,
                product: Arc::clone(&self.product),
            }) as Box<dyn AgentPersistenceTransaction>)
        })
    }
}

impl AgentPersistenceTransaction for LegacyAgentTransaction {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            DataRetentionRepository
                .database_utc_now(&self.transaction)
                .await
        })
    }

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, AgentTenantRecord> {
        Box::pin(async move {
            ServiceAccountRepository
                .lock_tenant_in_txn(&self.transaction, tenant_id, ServiceAccountLock::Share)
                .await
                .map(tenant_record)
        })
    }

    fn lock_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<AgentAccountRecord>> {
        Box::pin(async move {
            ServiceAccountRepository
                .find_by_id_in_txn(
                    &self.transaction,
                    tenant_id,
                    account_id,
                    ServiceAccountLock::Share,
                )
                .await
                .map(|account| account.map(account_record))
        })
    }

    fn lock_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        key_id: &'a str,
    ) -> PersistenceFuture<'a, Option<AgentCredentialRecord>> {
        Box::pin(async move {
            ServiceCredentialRepository
                .find_by_key_id_for_share(&self.transaction, tenant_id, account_id, key_id)
                .await
                .map(|credential| credential.map(credential_record))
        })
    }

    fn lock_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        delegation_id: i64,
    ) -> PersistenceFuture<'a, Option<AgentDelegationRecord>> {
        Box::pin(async move {
            let Some(delegation) = ServiceDelegationRepository
                .find_by_id_for_share(&self.transaction, tenant_id, delegation_id)
                .await?
            else {
                return Ok(None);
            };
            let capability_keys = ServiceDelegationRepository
                .capability_keys_for_share(&self.transaction, tenant_id, delegation.id)
                .await?
                .into_iter()
                .collect::<BTreeSet<_>>();
            Ok(Some(delegation_record(delegation, capability_keys)))
        })
    }

    fn require_capability<'a>(
        &'a self,
        tenant_id: &'a str,
        capability_code: &'a str,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            let snapshot = ProductRepository
                .tenant_product(&self.transaction, tenant_id)
                .await?
                .map(crate::legacy_product_persistence::tenant_snapshot)
                .ok_or_else(|| ryframe_kernel::AppError::NotFound("租户不存在".into()))?;
            self.product
                .require_capability_snapshot(snapshot, capability_code)
                .map(|_| ())
        })
    }

    fn authorization_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        represented_user_id: Option<i64>,
    ) -> PersistenceFuture<'a, AgentAuthorizationSnapshot> {
        Box::pin(async move {
            ServiceAuthorizationRepository
                .lock_snapshot_in_txn(
                    &self.transaction,
                    tenant_id,
                    account_id,
                    represented_user_id,
                )
                .await
                .map(crate::legacy_agent_snapshot::authorization_snapshot)
        })
    }

    fn users_page<'a>(
        &'a self,
        tenant_id: &'a str,
        scope: AgentRowScope,
        offset: u64,
        limit: u64,
    ) -> PersistenceFuture<'a, AgentQueryPage<AgentUserRecord>> {
        Box::pin(async move {
            AgentQueryRepository
                .users_page(
                    &self.transaction,
                    tenant_id,
                    &crate::legacy_agent_snapshot::row_scope(scope),
                    offset,
                    limit,
                )
                .await
                .map(|page| AgentQueryPage {
                    records: page.records.into_iter().map(user_record).collect(),
                    total: page.total,
                })
        })
    }

    fn departments_page<'a>(
        &'a self,
        tenant_id: &'a str,
        scope: AgentRowScope,
        offset: u64,
        limit: u64,
    ) -> PersistenceFuture<'a, AgentQueryPage<AgentDepartmentRecord>> {
        Box::pin(async move {
            AgentQueryRepository
                .departments_page(
                    &self.transaction,
                    tenant_id,
                    &crate::legacy_agent_snapshot::row_scope(scope),
                    offset,
                    limit,
                )
                .await
                .map(|page| AgentQueryPage {
                    records: page.records.into_iter().map(department_record).collect(),
                    total: page.total,
                })
        })
    }

    fn posts_page<'a>(
        &'a self,
        tenant_id: &'a str,
        offset: u64,
        limit: u64,
    ) -> PersistenceFuture<'a, AgentQueryPage<AgentPostRecord>> {
        Box::pin(async move {
            AgentQueryRepository
                .posts_page(&self.transaction, tenant_id, offset, limit)
                .await
                .map(|page| AgentQueryPage {
                    records: page.records.into_iter().map(post_record).collect(),
                    total: page.total,
                })
        })
    }

    fn dictionary_page<'a>(
        &'a self,
        tenant_id: &'a str,
        type_code: &'a str,
        offset: u64,
        limit: u64,
    ) -> PersistenceFuture<'a, Option<AgentDictionaryPageRecord>> {
        Box::pin(async move {
            AgentQueryRepository
                .dictionary_by_type_code_page(
                    &self.transaction,
                    tenant_id,
                    type_code,
                    offset,
                    limit,
                )
                .await
                .map(|page| {
                    page.map(|page| AgentDictionaryPageRecord {
                        type_code: page.dict_type.code,
                        records: page
                            .records
                            .into_iter()
                            .map(dictionary_item_record)
                            .collect(),
                        total: page.total,
                    })
                })
        })
    }

    fn insert_audit(&self, audit: AgentAccessAuditRecord) -> PersistenceFuture<'_, ()> {
        Box::pin(async move {
            ServiceAccessAuditRepository
                .insert(&self.transaction, crate::legacy_agent_audit::model(audit))
                .await
                .map(|_| ())
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.commit().await.map_err(database_error) })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn tenant_record(tenant: tenant::Model) -> AgentTenantRecord {
    AgentTenantRecord {
        tenant_id: tenant.tenant_id,
        status: tenant.status,
        expire_at: tenant.expire_at,
        authorization_epoch: tenant.authorization_epoch,
    }
}

fn account_record(account: service_account::Model) -> AgentAccountRecord {
    AgentAccountRecord {
        id: account.id,
        tenant_id: account.tenant_id,
        dept_id: account.dept_id,
        status: account.status,
        deleted: account.del_flag != service_account::Model::DEL_FLAG_NORMAL,
        authorization_version: account.authorization_version,
    }
}

fn credential_record(credential: service_credential::Model) -> AgentCredentialRecord {
    AgentCredentialRecord {
        id: credential.id,
        tenant_id: credential.tenant_id,
        account_id: credential.account_id,
        key_id: credential.key_id,
        secret_mac: credential.secret_mac,
        pepper_version: credential.pepper_version,
        status: credential.status,
        expires_at: credential.expires_at,
        revoked_at: credential.revoked_at,
    }
}

fn delegation_record(
    delegation: service_delegation::Model,
    capability_keys: BTreeSet<String>,
) -> AgentDelegationRecord {
    AgentDelegationRecord {
        id: delegation.id,
        tenant_id: delegation.tenant_id,
        account_id: delegation.account_id,
        user_id: delegation.user_id,
        token_mac: delegation.token_mac,
        pepper_version: delegation.pepper_version,
        status: delegation.status,
        version: delegation.version,
        not_before: delegation.not_before,
        expires_at: delegation.expires_at,
        revoked_at: delegation.revoked_at,
        capability_keys,
    }
}

fn user_record(user: user::Model) -> AgentUserRecord {
    AgentUserRecord {
        id: user.id,
        username: user.username,
        nickname: user.nickname,
        dept_id: user.dept_id,
        status: user.status,
    }
}

fn department_record(department: dept::Model) -> AgentDepartmentRecord {
    AgentDepartmentRecord {
        id: department.id,
        name: department.name,
        parent_id: department.parent_id,
        status: department.status,
    }
}

fn post_record(post: post::Model) -> AgentPostRecord {
    AgentPostRecord {
        id: post.id,
        code: post.code,
        name: post.name,
        status: post.status,
    }
}

fn dictionary_item_record(item: dict_data::Model) -> AgentDictionaryItemRecord {
    AgentDictionaryItemRecord {
        label: item.label,
        value: item.value,
        sort: item.sort,
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
