use chrono::{DateTime, Utc};

use crate::PersistenceFuture;

#[derive(Clone, Debug)]
pub struct IdentityTenantRecord {
    pub tenant_id: String,
    pub name: String,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_requests_per_min: i32,
    pub session_version: i32,
    pub authorization_epoch: i32,
}

impl IdentityTenantRecord {
    pub fn is_available(&self, now: DateTime<Utc>) -> bool {
        self.status == "enabled" && self.expire_at.is_none_or(|value| value > now)
    }
}

#[derive(Clone, Debug)]
pub struct IdentityUserRecord {
    pub id: i64,
    pub tenant_id: String,
    pub username: String,
    pub password_hash: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub preferred_locale: Option<String>,
    pub status: String,
    pub authorization_version: i32,
    pub dept_id: Option<i64>,
}

impl IdentityUserRecord {
    pub fn is_enabled(&self) -> bool {
        self.status == "1"
    }
}

#[derive(Clone, Debug)]
pub struct IdentityRoleRecord {
    pub id: i64,
    pub code: String,
    pub is_super: bool,
    pub data_scope: String,
}

pub trait IdentityAuthorizationReadPort: Send + Sync {
    fn tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<IdentityTenantRecord>>;

    fn user_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<IdentityUserRecord>>;

    fn user_by_username<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
    ) -> PersistenceFuture<'a, Option<IdentityUserRecord>>;

    fn roles<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Vec<IdentityRoleRecord>>;

    fn permission_codes<'a>(
        &'a self,
        tenant_id: &'a str,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<String>>;

    fn department_name<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> PersistenceFuture<'a, Option<String>>;

    fn department_ancestors<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> PersistenceFuture<'a, Option<String>>;

    fn role_department_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<i64>>;

    fn child_department_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> PersistenceFuture<'a, Vec<i64>>;
}
