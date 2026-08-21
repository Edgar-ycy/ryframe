use std::{collections::HashSet, sync::Arc};

use crate::{
    ControlDatabaseCluster, RoleRepository, ServiceAccountLock, ServiceAccountRepository,
    ServiceCredentialRepository, ServiceDelegationRepository, UserRepository,
    entities::{
        dept, permission, role, role_permission, service_account, service_credential,
        service_delegation,
    },
};
use ryframe_kernel::AppError;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter, QuerySelect,
    TransactionTrait, sea_query::LockType,
};

use super::control_transaction::DatabasePortTransaction;

use ryframe_application::{
    AuthorizationMirrorTransaction, PersistenceFuture, ServiceAccountPermissionSnapshot,
    ServiceAccountRecord, ServiceAccountUserRecord, ServiceAccountWritePort,
    ServiceAccountWriteTransaction, ServiceCredentialWriteRecord, ServiceDelegationIdentity,
    ServiceDelegationWriteRecord,
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

impl ServiceAccountWritePort for DatabaseServiceAccountWrite {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ServiceAccountWriteTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseServiceAccountWriteTransaction {
                transaction: transaction.into(),
            }) as Box<dyn ServiceAccountWriteTransaction>)
        })
    }
}

impl ServiceAccountWriteTransaction for DatabaseServiceAccountWriteTransaction {
    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction {
        &self.transaction
    }

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            ServiceAccountRepository
                .lock_tenant_in_txn(&self.transaction, tenant_id, ServiceAccountLock::Update)
                .await
                .map(|_| ())
        })
    }

    fn account_code_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            service_account::Entity::find()
                .filter(service_account::Column::TenantId.eq(tenant_id))
                .filter(service_account::Column::Code.eq(code))
                .one(&self.transaction)
                .await
                .map(|account| account.is_some())
                .map_err(database_error)
        })
    }

    fn department_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            dept::Entity::find_by_id(dept_id)
                .filter(dept::Column::TenantId.eq(tenant_id))
                .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Share)
                .one(&self.transaction)
                .await
                .map(|department| department.is_some())
                .map_err(database_error)
        })
    }

    fn lock_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceAccountRecord>> {
        Box::pin(async move {
            ServiceAccountRepository
                .find_by_id_in_txn(
                    &self.transaction,
                    tenant_id,
                    account_id,
                    ServiceAccountLock::Update,
                )
                .await
                .map(|account| account.map(account_record))
        })
    }

    fn insert_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account: ServiceAccountRecord,
    ) -> PersistenceFuture<'a, ServiceAccountRecord> {
        Box::pin(async move {
            ServiceAccountRepository
                .insert_in_txn(&self.transaction, tenant_id, account_model(account))
                .await
                .map(account_record)
        })
    }

    fn save_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account: ServiceAccountRecord,
    ) -> PersistenceFuture<'a, ServiceAccountRecord> {
        Box::pin(async move {
            ServiceAccountRepository
                .update_in_txn(&self.transaction, tenant_id, account_model(account))
                .await
                .map(account_record)
        })
    }

    fn replace_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            ServiceAccountRepository
                .replace_roles_in_txn(&self.transaction, tenant_id, account_id, role_ids)
                .await
        })
    }

    fn find_idempotent_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        idempotency_key_hash: &'a [u8],
    ) -> PersistenceFuture<'a, Option<ServiceCredentialWriteRecord>> {
        Box::pin(async move {
            ServiceCredentialRepository
                .find_idempotent(
                    &self.transaction,
                    tenant_id,
                    account_id,
                    idempotency_key_hash,
                )
                .await
                .map(|credential| credential.map(credential_record))
        })
    }

    fn count_active_credentials_at<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            ServiceCredentialRepository
                .count_active_at(&self.transaction, tenant_id, account_id, now)
                .await
        })
    }

    fn insert_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        credential: ServiceCredentialWriteRecord,
    ) -> PersistenceFuture<'a, ServiceCredentialWriteRecord> {
        Box::pin(async move {
            ServiceCredentialRepository
                .insert_in_txn(
                    &self.transaction,
                    tenant_id,
                    account_id,
                    credential_model(credential),
                )
                .await
                .map(credential_record)
        })
    }

    fn lock_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        credential_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceCredentialWriteRecord>> {
        Box::pin(async move {
            service_credential::Entity::find_by_id(credential_id)
                .filter(service_credential::Column::TenantId.eq(tenant_id))
                .filter(service_credential::Column::AccountId.eq(account_id))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map(|credential| credential.map(credential_record))
                .map_err(database_error)
        })
    }

    fn save_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        credential: ServiceCredentialWriteRecord,
    ) -> PersistenceFuture<'a, ServiceCredentialWriteRecord> {
        Box::pin(async move {
            if credential.tenant_id != tenant_id || credential.account_id != account_id {
                return Err(AppError::Authorization("凭据租户或服务账号不匹配".into()));
            }
            service_credential::ActiveModel::from(credential_model(credential))
                .update(&self.transaction)
                .await
                .map(credential_record)
                .map_err(database_error)
        })
    }

    fn lock_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceAccountUserRecord>> {
        Box::pin(async move {
            UserRepository
                .find_by_id_for_update(&self.transaction, tenant_id, user_id)
                .await
                .map(|user| {
                    user.map(|user| ServiceAccountUserRecord {
                        status: user.status,
                    })
                })
        })
    }

    fn permission_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        user_id: i64,
    ) -> PersistenceFuture<'a, ServiceAccountPermissionSnapshot> {
        Box::pin(async move {
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
        })
    }

    fn find_idempotent_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        idempotency_key_hash: &'a [u8],
    ) -> PersistenceFuture<'a, Option<ServiceDelegationWriteRecord>> {
        Box::pin(async move {
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
        })
    }

    fn delegation_identity<'a>(
        &'a self,
        tenant_id: &'a str,
        delegation_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceDelegationIdentity>> {
        Box::pin(async move {
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
        })
    }

    fn lock_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        delegation_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceDelegationWriteRecord>> {
        Box::pin(async move {
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
        })
    }

    fn insert_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        mut delegation: ServiceDelegationWriteRecord,
    ) -> PersistenceFuture<'a, ServiceDelegationWriteRecord> {
        Box::pin(async move {
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
                .replace_capabilities_in_txn(
                    &self.transaction,
                    tenant_id,
                    saved.id,
                    &capability_keys,
                )
                .await?;
            Ok(delegation_record(saved, capability_keys))
        })
    }

    fn save_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        mut delegation: ServiceDelegationWriteRecord,
    ) -> PersistenceFuture<'a, ServiceDelegationWriteRecord> {
        Box::pin(async move {
            if delegation.tenant_id != tenant_id {
                return Err(AppError::Authorization("委托租户不匹配".into()));
            }
            let capability_keys = std::mem::take(&mut delegation.capability_keys);
            service_delegation::ActiveModel::from(delegation_model(delegation))
                .update(&self.transaction)
                .await
                .map(|saved| delegation_record(saved, capability_keys))
                .map_err(database_error)
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move {
            super::audit_persistence::commit_current_audit(self.transaction.into_inner()).await
        })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn account_record(account: service_account::Model) -> ServiceAccountRecord {
    ServiceAccountRecord {
        id: account.id,
        tenant_id: account.tenant_id,
        code: account.code,
        name: account.name,
        description: account.description,
        dept_id: account.dept_id,
        status: account.status,
        authorization_version: account.authorization_version,
        max_requests_per_minute: account.max_requests_per_minute,
        created_by: account.created_by,
        deleted: account.del_flag == service_account::Model::DEL_FLAG_DELETED,
        created_at: account.created_at,
        updated_at: account.updated_at,
    }
}

fn account_model(account: ServiceAccountRecord) -> service_account::Model {
    service_account::Model {
        id: account.id,
        tenant_id: account.tenant_id,
        code: account.code,
        name: account.name,
        description: account.description,
        dept_id: account.dept_id,
        status: account.status,
        authorization_version: account.authorization_version,
        max_requests_per_minute: account.max_requests_per_minute,
        created_by: account.created_by,
        del_flag: if account.deleted {
            service_account::Model::DEL_FLAG_DELETED
        } else {
            service_account::Model::DEL_FLAG_NORMAL
        }
        .to_owned(),
        created_at: account.created_at,
        updated_at: account.updated_at,
    }
}

fn credential_record(credential: service_credential::Model) -> ServiceCredentialWriteRecord {
    ServiceCredentialWriteRecord {
        id: credential.id,
        tenant_id: credential.tenant_id,
        account_id: credential.account_id,
        key_id: credential.key_id,
        secret_mac: credential.secret_mac,
        pepper_version: credential.pepper_version,
        label: credential.label,
        status: credential.status,
        expires_at: credential.expires_at,
        last_used_at: credential.last_used_at,
        created_by: credential.created_by,
        revoked_at: credential.revoked_at,
        revoked_by: credential.revoked_by,
        created_at: credential.created_at,
        updated_at: credential.updated_at,
        idempotency_key_hash: credential.idempotency_key_hash,
        request_fingerprint: credential.request_fingerprint,
    }
}

fn credential_model(credential: ServiceCredentialWriteRecord) -> service_credential::Model {
    service_credential::Model {
        id: credential.id,
        tenant_id: credential.tenant_id,
        account_id: credential.account_id,
        key_id: credential.key_id,
        secret_mac: credential.secret_mac,
        pepper_version: credential.pepper_version,
        label: credential.label,
        status: credential.status,
        expires_at: credential.expires_at,
        last_used_at: credential.last_used_at,
        created_by: credential.created_by,
        revoked_at: credential.revoked_at,
        revoked_by: credential.revoked_by,
        created_at: credential.created_at,
        updated_at: credential.updated_at,
        idempotency_key_hash: credential.idempotency_key_hash,
        request_fingerprint: credential.request_fingerprint,
    }
}

fn delegation_record(
    delegation: service_delegation::Model,
    capability_keys: Vec<String>,
) -> ServiceDelegationWriteRecord {
    ServiceDelegationWriteRecord {
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
        reason: delegation.reason,
        created_by_user_id: delegation.created_by_user_id,
        revoked_at: delegation.revoked_at,
        revoked_by: delegation.revoked_by,
        created_at: delegation.created_at,
        updated_at: delegation.updated_at,
        idempotency_key_hash: delegation.idempotency_key_hash,
        request_fingerprint: delegation.request_fingerprint,
        capability_keys,
    }
}

fn delegation_model(delegation: ServiceDelegationWriteRecord) -> service_delegation::Model {
    service_delegation::Model {
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
        reason: delegation.reason,
        created_by_user_id: delegation.created_by_user_id,
        revoked_at: delegation.revoked_at,
        revoked_by: delegation.revoked_by,
        created_at: delegation.created_at,
        updated_at: delegation.updated_at,
        idempotency_key_hash: delegation.idempotency_key_hash,
        request_fingerprint: delegation.request_fingerprint,
    }
}

async fn enabled_role_ids<I>(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    role_ids: I,
) -> Result<Vec<i64>, AppError>
where
    I: IntoIterator<Item = i64>,
{
    let role_ids = role_ids.into_iter().collect::<Vec<_>>();
    if role_ids.is_empty() {
        return Ok(Vec::new());
    }
    role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::Id.is_in(role_ids))
        .filter(role::Column::Status.eq(role::Model::STATUS_NORMAL))
        .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map(|roles| roles.into_iter().map(|role| role.id).collect())
        .map_err(database_error)
}

async fn permission_codes(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    role_ids: &[i64],
) -> Result<HashSet<String>, AppError> {
    if role_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let permission_ids = role_permission::Entity::find()
        .filter(role_permission::Column::TenantId.eq(tenant_id))
        .filter(role_permission::Column::RoleId.is_in(role_ids.iter().copied()))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| row.perm_id)
        .collect::<Vec<_>>();
    if permission_ids.is_empty() {
        return Ok(HashSet::new());
    }
    permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .filter(permission::Column::Id.is_in(permission_ids))
        .filter(permission::Column::Status.eq("1"))
        .all(transaction)
        .await
        .map(|permissions| {
            permissions
                .into_iter()
                .map(|permission| permission.code)
                .collect()
        })
        .map_err(database_error)
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn account_mapping_preserves_deleted_state() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
        let model = service_account::Model {
            id: 1,
            tenant_id: "tenant-a".into(),
            code: "billing".into(),
            name: "结算服务".into(),
            description: None,
            dept_id: None,
            status: service_account::Model::STATUS_DISABLED.into(),
            authorization_version: 2,
            max_requests_per_minute: 60,
            created_by: 3,
            del_flag: service_account::Model::DEL_FLAG_DELETED.into(),
            created_at: now,
            updated_at: now,
        };

        let record = account_record(model);
        assert!(record.deleted);
        assert_eq!(
            account_model(record).del_flag,
            service_account::Model::DEL_FLAG_DELETED
        );
    }

    #[test]
    fn credential_mapping_preserves_secret_metadata() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
        let model = service_credential::Model {
            id: 1,
            tenant_id: "tenant-a".into(),
            account_id: 2,
            key_id: "key-a".into(),
            secret_mac: vec![1, 2],
            pepper_version: 3,
            label: "自动化".into(),
            status: service_credential::Model::STATUS_ACTIVE.into(),
            expires_at: now,
            last_used_at: None,
            created_by: 4,
            revoked_at: None,
            revoked_by: None,
            created_at: now,
            updated_at: now,
            idempotency_key_hash: vec![5, 6],
            request_fingerprint: vec![7, 8],
        };

        let record = credential_record(model);
        assert_eq!(record.secret_mac, [1, 2]);
        assert_eq!(record.request_fingerprint, [7, 8]);
        let restored = credential_model(record);
        assert_eq!(restored.pepper_version, 3);
        assert_eq!(restored.idempotency_key_hash, [5, 6]);
    }

    #[test]
    fn delegation_mapping_preserves_capabilities_without_copying() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
        let model = service_delegation::Model {
            id: 1,
            tenant_id: "tenant-a".into(),
            account_id: 2,
            user_id: 3,
            token_mac: vec![1, 2],
            pepper_version: 4,
            status: service_delegation::Model::STATUS_ACTIVE.into(),
            version: 1,
            not_before: now,
            expires_at: now,
            reason: "排障".into(),
            created_by_user_id: 3,
            revoked_at: None,
            revoked_by: None,
            created_at: now,
            updated_at: now,
            idempotency_key_hash: vec![5, 6],
            request_fingerprint: vec![7, 8],
        };

        let record = delegation_record(model, vec!["system:user:list".into()]);
        assert_eq!(record.capability_keys, ["system:user:list"]);
        let mut record = record;
        let capability_keys = std::mem::take(&mut record.capability_keys);
        let restored = delegation_model(record);
        assert_eq!(restored.token_mac, [1, 2]);
        assert_eq!(capability_keys, ["system:user:list"]);
    }
}
