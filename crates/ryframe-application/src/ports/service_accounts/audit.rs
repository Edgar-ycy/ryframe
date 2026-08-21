use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::PersistenceFuture;

#[derive(Debug)]
pub struct ServiceAccessAuditRecord {
    pub id: i64,
    pub request_id: String,
    pub tenant_id: Option<String>,
    pub account_id: Option<i64>,
    pub credential_id: Option<i64>,
    pub delegation_id: Option<i64>,
    pub represented_user_id: Option<i64>,
    pub operation_id: String,
    pub capability_key: String,
    pub required_permission: String,
    pub access_mode: String,
    pub result: String,
    pub reason_code: String,
    pub http_status: i32,
    pub row_count: Option<i32>,
    pub response_bytes: Option<i64>,
    pub tenant_epoch: Option<i32>,
    pub account_authorization_version: Option<i32>,
    pub user_authorization_version: Option<i32>,
    pub delegation_version: Option<i32>,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

pub trait ServiceAccountAuditReadPort: Send + Sync {
    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<ServiceAccessAuditRecord>>;
}
