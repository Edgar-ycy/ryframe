use chrono::{DateTime, Utc};

use crate::{AuthorizationMirrorTransaction, PersistenceFuture, ServiceAccountRecord};

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

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait ServiceAccountWritePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ServiceAccountWriteTransaction>>;
}
