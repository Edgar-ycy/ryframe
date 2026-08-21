use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::{AgentAccessAuditRecord, AgentAuthorizationSnapshot, AgentRowScope};
use crate::PersistenceFuture;

#[derive(Debug)]
pub struct AgentTenantRecord {
    pub tenant_id: String,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub authorization_epoch: i32,
}

impl AgentTenantRecord {
    const STATUS_ENABLED: &'static str = "enabled";

    pub fn is_available(&self, now: DateTime<Utc>) -> bool {
        self.status == Self::STATUS_ENABLED
            && self.expire_at.is_none_or(|expire_at| expire_at > now)
    }
}

#[derive(Debug)]
pub struct AgentAccountRecord {
    pub id: i64,
    pub tenant_id: String,
    pub dept_id: Option<i64>,
    pub status: String,
    pub deleted: bool,
    pub authorization_version: i32,
}

impl AgentAccountRecord {
    const STATUS_NORMAL: &'static str = "1";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL && !self.deleted
    }
}

#[derive(Debug)]
pub struct AgentCredentialRecord {
    pub id: i64,
    pub tenant_id: String,
    pub account_id: i64,
    pub key_id: String,
    pub secret_mac: Vec<u8>,
    pub pepper_version: i32,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl AgentCredentialRecord {
    const STATUS_ACTIVE: &'static str = "active";

    pub fn is_usable_at(&self, now: DateTime<Utc>) -> bool {
        self.status == Self::STATUS_ACTIVE && self.revoked_at.is_none() && self.expires_at > now
    }
}

#[derive(Debug)]
pub struct AgentDelegationRecord {
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
    pub revoked_at: Option<DateTime<Utc>>,
    pub capability_keys: BTreeSet<String>,
}

impl AgentDelegationRecord {
    const STATUS_ACTIVE: &'static str = "active";

    pub fn is_usable_at(&self, now: DateTime<Utc>) -> bool {
        self.status == Self::STATUS_ACTIVE
            && self.revoked_at.is_none()
            && self.not_before <= now
            && self.expires_at > now
    }
}

#[derive(Debug)]
pub struct AgentQueryPage<T> {
    pub records: Vec<T>,
    pub total: u64,
}

#[derive(Debug)]
pub struct AgentUserRecord {
    pub id: i64,
    pub username: String,
    pub nickname: String,
    pub dept_id: Option<i64>,
    pub status: String,
}

#[derive(Debug)]
pub struct AgentDepartmentRecord {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub status: String,
}

#[derive(Debug)]
pub struct AgentPostRecord {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub status: String,
}

#[derive(Debug)]
pub struct AgentDictionaryItemRecord {
    pub label: String,
    pub value: String,
    pub sort: i32,
}

#[derive(Debug)]
pub struct AgentDictionaryPageRecord {
    pub type_code: String,
    pub records: Vec<AgentDictionaryItemRecord>,
    pub total: u64,
}

pub trait AgentPersistenceTransaction: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, AgentTenantRecord>;

    fn lock_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<AgentAccountRecord>>;

    fn lock_credential<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        key_id: &'a str,
    ) -> PersistenceFuture<'a, Option<AgentCredentialRecord>>;

    fn lock_delegation<'a>(
        &'a self,
        tenant_id: &'a str,
        delegation_id: i64,
    ) -> PersistenceFuture<'a, Option<AgentDelegationRecord>>;

    fn require_capability<'a>(
        &'a self,
        tenant_id: &'a str,
        capability_code: &'a str,
    ) -> PersistenceFuture<'a, ()>;

    fn authorization_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        represented_user_id: Option<i64>,
    ) -> PersistenceFuture<'a, AgentAuthorizationSnapshot>;

    fn users_page<'a>(
        &'a self,
        tenant_id: &'a str,
        scope: AgentRowScope,
        offset: u64,
        limit: u64,
    ) -> PersistenceFuture<'a, AgentQueryPage<AgentUserRecord>>;

    fn departments_page<'a>(
        &'a self,
        tenant_id: &'a str,
        scope: AgentRowScope,
        offset: u64,
        limit: u64,
    ) -> PersistenceFuture<'a, AgentQueryPage<AgentDepartmentRecord>>;

    fn posts_page<'a>(
        &'a self,
        tenant_id: &'a str,
        offset: u64,
        limit: u64,
    ) -> PersistenceFuture<'a, AgentQueryPage<AgentPostRecord>>;

    fn dictionary_page<'a>(
        &'a self,
        tenant_id: &'a str,
        type_code: &'a str,
        offset: u64,
        limit: u64,
    ) -> PersistenceFuture<'a, Option<AgentDictionaryPageRecord>>;

    fn insert_audit(&self, audit: AgentAccessAuditRecord) -> PersistenceFuture<'_, ()>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait AgentPersistencePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn AgentPersistenceTransaction>>;
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 21, 8, 0, 0).unwrap()
    }

    #[test]
    fn tenant_expiration_is_fail_closed_at_boundary() {
        let current = now();
        let tenant = AgentTenantRecord {
            tenant_id: "tenant-a".into(),
            status: AgentTenantRecord::STATUS_ENABLED.into(),
            expire_at: Some(current),
            authorization_epoch: 1,
        };

        assert!(!tenant.is_available(current));
    }

    #[test]
    fn deleted_account_is_not_enabled() {
        let account = AgentAccountRecord {
            id: 1,
            tenant_id: "tenant-a".into(),
            dept_id: None,
            status: AgentAccountRecord::STATUS_NORMAL.into(),
            deleted: true,
            authorization_version: 1,
        };

        assert!(!account.is_enabled());
    }

    #[test]
    fn revoked_credential_is_not_usable() {
        let current = now();
        let credential = AgentCredentialRecord {
            id: 1,
            tenant_id: "tenant-a".into(),
            account_id: 2,
            key_id: "key-a".into(),
            secret_mac: vec![1],
            pepper_version: 1,
            status: AgentCredentialRecord::STATUS_ACTIVE.into(),
            expires_at: current + Duration::minutes(1),
            revoked_at: Some(current),
        };

        assert!(!credential.is_usable_at(current));
    }

    #[test]
    fn delegation_not_before_and_expiration_are_both_enforced() {
        let current = now();
        let delegation = AgentDelegationRecord {
            id: 1,
            tenant_id: "tenant-a".into(),
            account_id: 2,
            user_id: 3,
            token_mac: vec![1],
            pepper_version: 1,
            status: AgentDelegationRecord::STATUS_ACTIVE.into(),
            version: 1,
            not_before: current + Duration::seconds(1),
            expires_at: current + Duration::minutes(1),
            revoked_at: None,
            capability_keys: BTreeSet::new(),
        };

        assert!(!delegation.is_usable_at(current));
        assert!(delegation.is_usable_at(current + Duration::seconds(1)));
        assert!(!delegation.is_usable_at(current + Duration::minutes(1)));
    }
}
