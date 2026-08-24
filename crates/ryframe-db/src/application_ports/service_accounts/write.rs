use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, RoleRepository, ServiceAccountLock, ServiceAccountRepository,
    ServiceCredentialRepository, ServiceDelegationRepository, UserRepository,
    entities::{dept, role, service_account, service_credential, service_delegation},
};
use ryframe_kernel::AppError;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait,
    sea_query::LockType,
};

mod support;

pub use support::{
    account_model, credential_model, credential_record, delegation_model, delegation_record,
};
use support::{database_error, enabled_role_ids, permission_codes};

use super::{super::transaction::DatabasePortTransaction, account_record};

use ryframe_application::{
    ports::authorization::AuthorizationMirrorTransaction,
    ports::service_accounts::{
        ServiceAccountPermissionSnapshot, ServiceAccountRecord, ServiceAccountUserRecord,
        ServiceAccountWritePort, ServiceAccountWriteTransaction, ServiceCredentialWriteRecord,
        ServiceDelegationIdentity, ServiceDelegationWriteRecord,
    },
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ServiceAccountWritePort> {
    Arc::new(DatabaseServiceAccountWrite { database })
}

struct DatabaseServiceAccountWrite {
    database: ControlDatabaseCluster,
}

struct DatabaseServiceAccountWriteTransaction {
    transaction: DatabasePortTransaction,
}

#[async_trait::async_trait]
impl ServiceAccountWritePort for DatabaseServiceAccountWrite {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ServiceAccountWriteTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseServiceAccountWriteTransaction {
            transaction: transaction.into(),
        }) as Box<dyn ServiceAccountWriteTransaction>)
    }
}

#[async_trait::async_trait]
impl ServiceAccountWriteTransaction for DatabaseServiceAccountWriteTransaction {
    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction {
        &self.transaction
    }

    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()> {
        ServiceAccountRepository
            .lock_tenant_in_txn(&self.transaction, tenant_id, ServiceAccountLock::Update)
            .await
            .map(|_| ())
    }

    async fn account_code_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> ryframe_kernel::AppResult<bool> {
        service_account::Entity::find()
            .filter(service_account::Column::TenantId.eq(tenant_id))
            .filter(service_account::Column::Code.eq(code))
            .one(&self.transaction)
            .await
            .map(|account| account.is_some())
            .map_err(database_error)
    }

    async fn department_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> ryframe_kernel::AppResult<bool> {
        dept::Entity::find_by_id(dept_id)
            .filter(dept::Column::TenantId.eq(tenant_id))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Share)
            .one(&self.transaction)
            .await
            .map(|department| department.is_some())
            .map_err(database_error)
    }

    async fn lock_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ServiceAccountRecord>> {
        ServiceAccountRepository
            .find_by_id_in_txn(
                &self.transaction,
                tenant_id,
                account_id,
                ServiceAccountLock::Update,
            )
            .await
            .map(|account| account.map(account_record))
    }

    async fn insert_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account: ServiceAccountRecord,
    ) -> ryframe_kernel::AppResult<ServiceAccountRecord> {
        ServiceAccountRepository
            .insert_in_txn(&self.transaction, tenant_id, account_model(account))
            .await
            .map(account_record)
    }

    async fn save_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account: ServiceAccountRecord,
    ) -> ryframe_kernel::AppResult<ServiceAccountRecord> {
        ServiceAccountRepository
            .update_in_txn(&self.transaction, tenant_id, account_model(account))
            .await
            .map(account_record)
    }

    async fn replace_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        role_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<()> {
        ServiceAccountRepository
            .replace_roles_in_txn(&self.transaction, tenant_id, account_id, role_ids)
            .await
    }

    async fn find_idempotent_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        idempotency_key_hash: &'a [u8],
    ) -> ryframe_kernel::AppResult<Option<ServiceCredentialWriteRecord>> {
        ServiceCredentialRepository
            .find_idempotent(
                &self.transaction,
                tenant_id,
                account_id,
                idempotency_key_hash,
            )
            .await
            .map(|credential| credential.map(credential_record))
    }

    async fn count_active_credentials_at<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<u64> {
        ServiceCredentialRepository
            .count_active_at(&self.transaction, tenant_id, account_id, now)
            .await
    }

    async fn insert_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        credential: ServiceCredentialWriteRecord,
    ) -> ryframe_kernel::AppResult<ServiceCredentialWriteRecord> {
        ServiceCredentialRepository
            .insert_in_txn(
                &self.transaction,
                tenant_id,
                account_id,
                credential_model(credential),
            )
            .await
            .map(credential_record)
    }

    async fn lock_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        credential_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ServiceCredentialWriteRecord>> {
        service_credential::Entity::find_by_id(credential_id)
            .filter(service_credential::Column::TenantId.eq(tenant_id))
            .filter(service_credential::Column::AccountId.eq(account_id))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map(|credential| credential.map(credential_record))
            .map_err(database_error)
    }

    async fn save_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        credential: ServiceCredentialWriteRecord,
    ) -> ryframe_kernel::AppResult<ServiceCredentialWriteRecord> {
        if credential.tenant_id != tenant_id || credential.account_id != account_id {
            return Err(AppError::Authorization("凭据租户或服务账号不匹配".into()));
        }
        service_credential::ActiveModel::from(credential_model(credential))
            .update(&self.transaction)
            .await
            .map(credential_record)
            .map_err(database_error)
    }

    async fn lock_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ServiceAccountUserRecord>> {
        UserRepository
            .find_by_id_for_update(&self.transaction, tenant_id, user_id)
            .await
            .map(|user| {
                user.map(|user| ServiceAccountUserRecord {
                    status: user.status,
                })
            })
    }

    async fn permission_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<ServiceAccountPermissionSnapshot> {
        let user_role_ids = RoleRepository
            .find_user_roles_all_status(&self.transaction, tenant_id, user_id)
            .await?
            .into_iter()
            .filter(|role| role.status == role::Model::STATUS_NORMAL)
            .map(|role| role.id)
            .collect::<Vec<_>>();
        let account_role_ids = ServiceAccountRepository
            .role_ids(&self.transaction, tenant_id, account_id)
            .await?;
        let enabled_account_role_ids = enabled_role_ids(
            &self.transaction,
            tenant_id,
            account_role_ids.iter().copied(),
        )
        .await?;
        Ok(ServiceAccountPermissionSnapshot {
            user_permissions: permission_codes(&self.transaction, tenant_id, &user_role_ids)
                .await?,
            account_permissions: permission_codes(
                &self.transaction,
                tenant_id,
                &enabled_account_role_ids,
            )
            .await?,
        })
    }

    async fn find_idempotent_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        idempotency_key_hash: &'a [u8],
    ) -> ryframe_kernel::AppResult<Option<ServiceDelegationWriteRecord>> {
        let Some(delegation) = ServiceDelegationRepository
            .find_idempotent(&self.transaction, tenant_id, user_id, idempotency_key_hash)
            .await?
        else {
            return Ok(None);
        };
        let capability_keys = ServiceDelegationRepository
            .capability_keys(&self.transaction, tenant_id, delegation.id)
            .await?;
        Ok(Some(delegation_record(delegation, capability_keys)))
    }

    async fn delegation_identity<'a>(
        &'a self,
        tenant_id: &'a str,
        delegation_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ServiceDelegationIdentity>> {
        service_delegation::Entity::find_by_id(delegation_id)
            .select_only()
            .columns([
                service_delegation::Column::AccountId,
                service_delegation::Column::UserId,
            ])
            .filter(service_delegation::Column::TenantId.eq(tenant_id))
            .into_tuple::<(i64, i64)>()
            .one(&self.transaction)
            .await
            .map(|identity| {
                identity.map(|(account_id, user_id)| ServiceDelegationIdentity {
                    account_id,
                    user_id,
                })
            })
            .map_err(database_error)
    }

    async fn lock_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        delegation_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ServiceDelegationWriteRecord>> {
        let Some(delegation) = service_delegation::Entity::find_by_id(delegation_id)
            .filter(service_delegation::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map_err(database_error)?
        else {
            return Ok(None);
        };
        let capability_keys = ServiceDelegationRepository
            .capability_keys(&self.transaction, tenant_id, delegation.id)
            .await?;
        Ok(Some(delegation_record(delegation, capability_keys)))
    }

    async fn insert_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        mut delegation: ServiceDelegationWriteRecord,
    ) -> ryframe_kernel::AppResult<ServiceDelegationWriteRecord> {
        let capability_keys = std::mem::take(&mut delegation.capability_keys);
        let saved = ServiceDelegationRepository
            .insert_in_txn(
                &self.transaction,
                tenant_id,
                user_id,
                delegation_model(delegation),
            )
            .await?;
        ServiceDelegationRepository
            .replace_capabilities_in_txn(&self.transaction, tenant_id, saved.id, &capability_keys)
            .await?;
        Ok(delegation_record(saved, capability_keys))
    }

    async fn save_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        mut delegation: ServiceDelegationWriteRecord,
    ) -> ryframe_kernel::AppResult<ServiceDelegationWriteRecord> {
        if delegation.tenant_id != tenant_id {
            return Err(AppError::Authorization("委托租户不匹配".into()));
        }
        let capability_keys = std::mem::take(&mut delegation.capability_keys);
        service_delegation::ActiveModel::from(delegation_model(delegation))
            .update(&self.transaction)
            .await
            .map(|saved| delegation_record(saved, capability_keys))
            .map_err(database_error)
    }
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for DatabaseServiceAccountWriteTransaction {
    async fn commit(
        self: Box<Self>,
        audit_mode: ryframe_application::TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        match audit_mode {
            ryframe_application::TransactionAuditMode::CurrentRequest => {
                self.transaction.commit_audited().await
            }
            ryframe_application::TransactionAuditMode::Skip => {
                self.transaction.commit().await.map_err(database_error)
            }
        }
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
    }
}
