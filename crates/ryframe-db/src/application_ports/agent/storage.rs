use crate::DbResultExt;
use std::{collections::BTreeSet, sync::Arc};

use crate::{
    AgentQueryRepository, ControlDatabaseCluster, ProductRepository, ServiceAccessAuditRepository,
    ServiceAccountLock, ServiceAccountRepository, ServiceAuthorizationRepository,
    ServiceCredentialRepository, ServiceDelegationRepository,
    entities::{
        dept, dict_data, service_account, service_credential, service_delegation, tenant, user,
    },
    generated::entities::post,
};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
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
    Arc::new(DatabaseAgentPersistence { database, product })
}

struct DatabaseAgentPersistence {
    database: ControlDatabaseCluster,
    product: Arc<ProductService>,
}

struct DatabaseAgentTransaction {
    transaction: DatabaseTransaction,
    product: Arc<ProductService>,
}

#[async_trait::async_trait]
impl AgentPersistencePort for DatabaseAgentPersistence {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn AgentPersistenceTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseAgentTransaction {
            transaction,
            product: Arc::clone(&self.product),
        }) as Box<dyn AgentPersistenceTransaction>)
    }
}

#[async_trait::async_trait]
impl AgentPersistenceTransaction for DatabaseAgentTransaction {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn lock_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<AgentTenantRecord> {
        ServiceAccountRepository
            .lock_tenant_in_txn(&self.transaction, tenant_id, ServiceAccountLock::Share)
            .await
            .map(tenant_record)
    }

    async fn lock_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> ryframe_kernel::AppResult<Option<AgentAccountRecord>> {
        ServiceAccountRepository
            .find_by_id_in_txn(
                &self.transaction,
                tenant_id,
                account_id,
                ServiceAccountLock::Share,
            )
            .await
            .map(|account| account.map(account_record))
    }

    async fn lock_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        key_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<AgentCredentialRecord>> {
        ServiceCredentialRepository
            .find_by_key_id_for_share(&self.transaction, tenant_id, account_id, key_id)
            .await
            .map(|credential| credential.map(credential_record))
    }

    async fn lock_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        delegation_id: i64,
    ) -> ryframe_kernel::AppResult<Option<AgentDelegationRecord>> {
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
    }

    async fn require_capability<'a>(
        &'a self,
        tenant_id: &'a str,
        capability_code: &'a str,
    ) -> ryframe_kernel::AppResult<()> {
        let snapshot = ProductRepository
            .tenant_product(&self.transaction, tenant_id)
            .await?
            .map(super::super::product::tenant_snapshot)
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        self.product
            .require_capability_snapshot(snapshot, capability_code)
            .map(|_| ())
    }

    async fn authorization_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        represented_user_id: Option<i64>,
    ) -> ryframe_kernel::AppResult<AgentAuthorizationSnapshot> {
        ServiceAuthorizationRepository
            .lock_snapshot_in_txn(
                &self.transaction,
                tenant_id,
                account_id,
                represented_user_id,
            )
            .await
            .map(super::mapping::authorization_snapshot)
    }

    async fn users_page<'a>(
        &'a self,
        tenant_id: &'a str,
        scope: AgentRowScope,
        offset: u64,
        limit: u64,
    ) -> ryframe_kernel::AppResult<AgentQueryPage<AgentUserRecord>> {
        AgentQueryRepository
            .users_page(
                &self.transaction,
                tenant_id,
                &super::mapping::row_scope(scope),
                offset,
                limit,
            )
            .await
            .map(|page| AgentQueryPage {
                records: page.records.into_iter().map(user_record).collect(),
                total: page.total,
            })
    }

    async fn departments_page<'a>(
        &'a self,
        tenant_id: &'a str,
        scope: AgentRowScope,
        offset: u64,
        limit: u64,
    ) -> ryframe_kernel::AppResult<AgentQueryPage<AgentDepartmentRecord>> {
        AgentQueryRepository
            .departments_page(
                &self.transaction,
                tenant_id,
                &super::mapping::row_scope(scope),
                offset,
                limit,
            )
            .await
            .map(|page| AgentQueryPage {
                records: page.records.into_iter().map(department_record).collect(),
                total: page.total,
            })
    }

    async fn posts_page<'a>(
        &'a self,
        tenant_id: &'a str,
        offset: u64,
        limit: u64,
    ) -> ryframe_kernel::AppResult<AgentQueryPage<AgentPostRecord>> {
        AgentQueryRepository
            .posts_page(&self.transaction, tenant_id, offset, limit)
            .await
            .map(|page| AgentQueryPage {
                records: page.records.into_iter().map(post_record).collect(),
                total: page.total,
            })
    }

    async fn dictionary_page<'a>(
        &'a self,
        tenant_id: &'a str,
        type_code: &'a str,
        offset: u64,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Option<AgentDictionaryPageRecord>> {
        AgentQueryRepository
            .dictionary_by_type_code_page(&self.transaction, tenant_id, type_code, offset, limit)
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
    }

    async fn insert_audit(&self, audit: AgentAccessAuditRecord) -> ryframe_kernel::AppResult<()> {
        ServiceAccessAuditRepository
            .insert(&self.transaction, super::audit::model(audit))
            .await
            .map(|_| ())
    }
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for DatabaseAgentTransaction {
    async fn commit(
        self: Box<Self>,
        audit_mode: ryframe_application::TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        let _ = audit_mode;
        self.transaction.commit().await.db()
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.db()
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
