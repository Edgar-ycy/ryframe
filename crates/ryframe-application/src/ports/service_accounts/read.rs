use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::PersistenceFuture;

#[derive(Debug)]
pub struct ServiceAccountRecord {
    pub id: i64,
    pub tenant_id: String,
    pub code: String,
    pub name: String,
    pub description: Option<String>,
    pub dept_id: Option<i64>,
    pub status: String,
    pub authorization_version: i32,
    pub max_requests_per_minute: i32,
    pub created_by: i64,
    pub deleted: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ServiceAccountRecord {
    pub const STATUS_NORMAL: &'static str = "1";
    pub const STATUS_DISABLED: &'static str = "0";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL
    }
}

#[derive(Debug)]
pub struct ServiceAccountDetailRecord {
    pub account: ServiceAccountRecord,
    pub role_ids: Vec<i64>,
}

#[derive(Debug)]
pub struct ServiceCredentialRecord {
    pub id: i64,
    pub account_id: i64,
    pub key_id: String,
    pub label: String,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct ServiceDelegationRecord {
    pub id: i64,
    pub account_id: i64,
    pub user_id: i64,
    pub status: String,
    pub version: i32,
    pub not_before: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub reason: String,
    pub capability_keys: Vec<String>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub trait ServiceAccountReadPort: Send + Sync {
    fn list_accounts<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<ServiceAccountRecord>>;

    fn account_detail<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceAccountDetailRecord>>;

    fn enabled_account_role_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<Vec<i64>>>;

    fn enabled_account_credentials<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<Vec<ServiceCredentialRecord>>>;

    fn delegations_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Vec<ServiceDelegationRecord>>;

    fn list_delegations<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<ServiceDelegationRecord>>;
}
