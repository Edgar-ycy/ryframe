use chrono::{DateTime, Utc};

use crate::{
    AuthorizationMirrorTransaction, PersistenceFuture, ServiceAccountPermissionSnapshot,
    ServiceAccountRecord,
};

#[derive(Debug)]
pub struct ServiceCredentialWriteRecord {
    pub id: i64,
    pub tenant_id: String,
    pub account_id: i64,
    pub key_id: String,
    pub secret_mac: Vec<u8>,
    pub pepper_version: i32,
    pub label: String,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_by: i64,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub idempotency_key_hash: Vec<u8>,
    pub request_fingerprint: Vec<u8>,
}

impl ServiceCredentialWriteRecord {
    pub const STATUS_ACTIVE: &'static str = "active";
    pub const STATUS_REVOKED: &'static str = "revoked";
}

#[derive(Debug)]
pub struct ServiceAccountUserRecord {
    pub status: String,
}

impl ServiceAccountUserRecord {
    pub const STATUS_NORMAL: &'static str = "1";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL
    }
}

#[derive(Debug)]
pub struct ServiceDelegationIdentity {
    pub account_id: i64,
    pub user_id: i64,
}

#[derive(Debug)]
pub struct ServiceDelegationWriteRecord {
    pub id: i64,
    pub tenant_id: String,
    pub account_id: i64,
    pub user_id: i64,
    pub token_mac: Vec<u8>,
    pub pepper_version: i32,
    pub status: String,
    pub version: i32,
    pub not_before: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub reason: String,
    pub created_by_user_id: i64,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub idempotency_key_hash: Vec<u8>,
    pub request_fingerprint: Vec<u8>,
    pub capability_keys: Vec<String>,
}

impl ServiceDelegationWriteRecord {
    pub const STATUS_ACTIVE: &'static str = "active";
    pub const STATUS_REVOKED: &'static str = "revoked";
}

pub trait ServiceAccountWriteTransaction: Send + Sync {
    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction;

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn account_code_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, bool>;

    fn department_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> PersistenceFuture<'a, bool>;

    fn lock_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceAccountRecord>>;

    fn insert_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account: ServiceAccountRecord,
    ) -> PersistenceFuture<'a, ServiceAccountRecord>;

    fn save_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account: ServiceAccountRecord,
    ) -> PersistenceFuture<'a, ServiceAccountRecord>;

    fn replace_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, ()>;

    fn find_idempotent_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        idempotency_key_hash: &'a [u8],
    ) -> PersistenceFuture<'a, Option<ServiceCredentialWriteRecord>>;

    fn count_active_credentials_at<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, u64>;

    fn insert_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        credential: ServiceCredentialWriteRecord,
    ) -> PersistenceFuture<'a, ServiceCredentialWriteRecord>;

    fn lock_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        credential_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceCredentialWriteRecord>>;

    fn save_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        credential: ServiceCredentialWriteRecord,
    ) -> PersistenceFuture<'a, ServiceCredentialWriteRecord>;

    fn lock_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceAccountUserRecord>>;

    fn permission_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        user_id: i64,
    ) -> PersistenceFuture<'a, ServiceAccountPermissionSnapshot>;

    fn find_idempotent_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        idempotency_key_hash: &'a [u8],
    ) -> PersistenceFuture<'a, Option<ServiceDelegationWriteRecord>>;

    fn delegation_identity<'a>(
        &'a self,
        tenant_id: &'a str,
        delegation_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceDelegationIdentity>>;

    fn lock_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        delegation_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceDelegationWriteRecord>>;

    fn insert_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        delegation: ServiceDelegationWriteRecord,
    ) -> PersistenceFuture<'a, ServiceDelegationWriteRecord>;

    fn save_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        delegation: ServiceDelegationWriteRecord,
    ) -> PersistenceFuture<'a, ServiceDelegationWriteRecord>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait ServiceAccountWritePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ServiceAccountWriteTransaction>>;
}
