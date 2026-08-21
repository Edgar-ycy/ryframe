use super::*;

/// 可由 Agent 注册表提供给管理域的稳定能力描述。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ServiceCapabilityDescriptor {
    pub key: String,
    pub permission: String,
    pub direct: bool,
    pub delegated: bool,
}

#[derive(Clone, Debug)]
pub struct CreateServiceAccountCommand {
    pub code: String,
    pub name: String,
    pub description: Option<String>,
    pub dept_id: Option<i64>,
    pub max_requests_per_minute: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct UpdateServiceAccountCommand {
    pub name: String,
    pub description: Option<String>,
    pub dept_id: Option<i64>,
    pub max_requests_per_minute: i32,
}

#[derive(Clone, Debug)]
pub struct CreateCredentialCommand {
    pub label: String,
    pub expires_at: DateTime<Utc>,
    pub idempotency_key: String,
}

#[derive(Clone, Debug)]
pub struct CreateDelegationCommand {
    pub account_id: i64,
    pub capability_keys: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub reason: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceAccountVo {
    pub id: String,
    pub code: String,
    pub name: String,
    pub description: Option<String>,
    pub dept_id: Option<String>,
    pub status: String,
    pub authorization_version: i32,
    pub max_requests_per_minute: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ServiceAccountRecord> for ServiceAccountVo {
    fn from(account: ServiceAccountRecord) -> Self {
        Self {
            id: account.id.to_string(),
            code: account.code,
            name: account.name,
            description: account.description,
            dept_id: account.dept_id.map(|id| id.to_string()),
            status: account.status,
            authorization_version: account.authorization_version,
            max_requests_per_minute: account.max_requests_per_minute,
            created_at: account.created_at,
            updated_at: account.updated_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceAccountDetailVo {
    #[serde(flatten)]
    pub account: ServiceAccountVo,
    pub role_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceCredentialVo {
    pub id: String,
    pub account_id: String,
    pub key_id: String,
    pub label: String,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<ServiceCredentialRecord> for ServiceCredentialVo {
    fn from(credential: ServiceCredentialRecord) -> Self {
        Self {
            id: credential.id.to_string(),
            account_id: credential.account_id.to_string(),
            key_id: format!("rfk_{}", credential.key_id),
            label: credential.label,
            status: credential.status,
            expires_at: credential.expires_at,
            last_used_at: credential.last_used_at,
            revoked_at: credential.revoked_at,
            created_at: credential.created_at,
        }
    }
}

impl From<ServiceCredentialWriteRecord> for ServiceCredentialVo {
    fn from(credential: ServiceCredentialWriteRecord) -> Self {
        Self {
            id: credential.id.to_string(),
            account_id: credential.account_id.to_string(),
            key_id: format!("rfk_{}", credential.key_id),
            label: credential.label,
            status: credential.status,
            expires_at: credential.expires_at,
            last_used_at: credential.last_used_at,
            revoked_at: credential.revoked_at,
            created_at: credential.created_at,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct CreatedCredentialVo {
    #[serde(flatten)]
    pub credential: ServiceCredentialVo,
    /// 只在首次成功创建时返回；幂等重放永远为 `None`。
    pub secret: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceDelegationVo {
    pub id: String,
    pub account_id: String,
    pub user_id: String,
    pub status: String,
    pub version: i32,
    pub not_before: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub reason: String,
    pub capability_keys: Vec<String>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<ServiceDelegationRecord> for ServiceDelegationVo {
    fn from(delegation: ServiceDelegationRecord) -> Self {
        Self {
            id: delegation.id.to_string(),
            account_id: delegation.account_id.to_string(),
            user_id: delegation.user_id.to_string(),
            status: delegation.status,
            version: delegation.version,
            not_before: delegation.not_before,
            expires_at: delegation.expires_at,
            reason: delegation.reason,
            capability_keys: delegation.capability_keys,
            revoked_at: delegation.revoked_at,
            created_at: delegation.created_at,
        }
    }
}

impl From<ServiceDelegationWriteRecord> for ServiceDelegationVo {
    fn from(delegation: ServiceDelegationWriteRecord) -> Self {
        Self {
            id: delegation.id.to_string(),
            account_id: delegation.account_id.to_string(),
            user_id: delegation.user_id.to_string(),
            status: delegation.status,
            version: delegation.version,
            not_before: delegation.not_before,
            expires_at: delegation.expires_at,
            reason: delegation.reason,
            capability_keys: delegation.capability_keys,
            revoked_at: delegation.revoked_at,
            created_at: delegation.created_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceDelegationTargetVo {
    pub account_id: String,
    pub code: String,
    pub name: String,
    pub capabilities: Vec<ServiceCapabilityDescriptor>,
}

#[derive(Clone, Serialize)]
pub struct CreatedDelegationVo {
    #[serde(flatten)]
    pub delegation: ServiceDelegationVo,
    /// 只在首次成功创建时返回；幂等重放永远为 `None`。
    pub token: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceAccessAuditVo {
    pub id: String,
    pub request_id: String,
    pub tenant_id: Option<String>,
    pub account_id: Option<String>,
    pub credential_id: Option<String>,
    pub delegation_id: Option<String>,
    pub represented_user_id: Option<String>,
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

impl From<crate::ports::service_accounts::ServiceAccessAuditRecord> for ServiceAccessAuditVo {
    fn from(audit: crate::ports::service_accounts::ServiceAccessAuditRecord) -> Self {
        Self {
            id: audit.id.to_string(),
            request_id: audit.request_id,
            tenant_id: audit.tenant_id,
            account_id: audit.account_id.map(|id| id.to_string()),
            credential_id: audit.credential_id.map(|id| id.to_string()),
            delegation_id: audit.delegation_id.map(|id| id.to_string()),
            represented_user_id: audit.represented_user_id.map(|id| id.to_string()),
            operation_id: audit.operation_id,
            capability_key: audit.capability_key,
            required_permission: audit.required_permission,
            access_mode: audit.access_mode,
            result: audit.result,
            reason_code: audit.reason_code,
            http_status: audit.http_status,
            row_count: audit.row_count,
            response_bytes: audit.response_bytes,
            tenant_epoch: audit.tenant_epoch,
            account_authorization_version: audit.account_authorization_version,
            user_authorization_version: audit.user_authorization_version,
            delegation_version: audit.delegation_version,
            started_at: audit.started_at,
            completed_at: audit.completed_at,
        }
    }
}
