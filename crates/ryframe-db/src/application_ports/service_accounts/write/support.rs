use std::collections::HashSet;

use ryframe_application::ports::service_accounts::{
    ServiceAccountRecord, ServiceCredentialWriteRecord, ServiceDelegationWriteRecord,
};
use ryframe_kernel::AppError;
use sea_orm::{ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter};

use crate::entities::{
    permission, role, role_permission, service_account, service_credential, service_delegation,
};

pub fn account_model(account: ServiceAccountRecord) -> service_account::Model {
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

pub fn credential_record(credential: service_credential::Model) -> ServiceCredentialWriteRecord {
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

pub fn credential_model(credential: ServiceCredentialWriteRecord) -> service_credential::Model {
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

pub fn delegation_record(
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

pub fn delegation_model(delegation: ServiceDelegationWriteRecord) -> service_delegation::Model {
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

pub(super) async fn enabled_role_ids<I>(
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

pub(super) async fn permission_codes(
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

pub(super) fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
