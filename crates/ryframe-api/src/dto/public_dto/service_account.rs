use chrono::{DateTime, Utc};
use ryframe_service::system::{
    CreatedCredentialVo as ServiceCreatedCredentialVo,
    CreatedDelegationVo as ServiceCreatedDelegationVo, ServiceAccessAuditVo as ServiceAccessAudit,
    ServiceAccountDetailVo as ServiceAccountDetail, ServiceAccountVo as ServiceAccount,
    ServiceCapabilityDescriptor as ServiceCapability, ServiceCredentialVo as ServiceCredential,
    ServiceDelegationVo as ServiceDelegation,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceCapabilityVo {
    pub key: String,
    pub permission: String,
    pub direct: bool,
    pub delegated: bool,
}

impl From<ServiceCapability> for ServiceCapabilityVo {
    fn from(value: ServiceCapability) -> Self {
        Self {
            key: value.key,
            permission: value.permission,
            direct: value.direct,
            delegated: value.delegated,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
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

impl From<ServiceAccount> for ServiceAccountVo {
    fn from(value: ServiceAccount) -> Self {
        Self {
            id: value.id,
            code: value.code,
            name: value.name,
            description: value.description,
            dept_id: value.dept_id,
            status: value.status,
            authorization_version: value.authorization_version,
            max_requests_per_minute: value.max_requests_per_minute,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceAccountDetailVo {
    pub account: ServiceAccountVo,
    pub role_ids: Vec<String>,
}

impl From<ServiceAccountDetail> for ServiceAccountDetailVo {
    fn from(value: ServiceAccountDetail) -> Self {
        Self {
            account: value.account.into(),
            role_ids: value.role_ids,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
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

impl From<ServiceCredential> for ServiceCredentialVo {
    fn from(value: ServiceCredential) -> Self {
        Self {
            id: value.id,
            account_id: value.account_id,
            key_id: value.key_id,
            label: value.label,
            status: value.status,
            expires_at: value.expires_at,
            last_used_at: value.last_used_at,
            revoked_at: value.revoked_at,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedServiceCredentialVo {
    pub credential: ServiceCredentialVo,
    /// 仅首次成功时返回完整 API Key；幂等重放为 `null`。
    pub secret: Option<String>,
}

impl From<ServiceCreatedCredentialVo> for CreatedServiceCredentialVo {
    fn from(value: ServiceCreatedCredentialVo) -> Self {
        Self {
            credential: value.credential.into(),
            secret: value.secret,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
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

impl From<ServiceDelegation> for ServiceDelegationVo {
    fn from(value: ServiceDelegation) -> Self {
        Self {
            id: value.id,
            account_id: value.account_id,
            user_id: value.user_id,
            status: value.status,
            version: value.version,
            not_before: value.not_before,
            expires_at: value.expires_at,
            reason: value.reason,
            capability_keys: value.capability_keys,
            revoked_at: value.revoked_at,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedServiceDelegationVo {
    pub delegation: ServiceDelegationVo,
    /// 仅首次成功时返回委托令牌；幂等重放为 `null`。
    pub token: Option<String>,
}

impl From<ServiceCreatedDelegationVo> for CreatedServiceDelegationVo {
    fn from(value: ServiceCreatedDelegationVo) -> Self {
        Self {
            delegation: value.delegation.into(),
            token: value.token,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
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
    pub tenant_epoch: Option<String>,
    pub account_authorization_version: Option<i32>,
    pub user_authorization_version: Option<i32>,
    pub delegation_version: Option<i32>,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

impl From<ServiceAccessAudit> for ServiceAccessAuditVo {
    fn from(value: ServiceAccessAudit) -> Self {
        Self {
            id: value.id,
            request_id: value.request_id,
            tenant_id: value.tenant_id,
            account_id: value.account_id,
            credential_id: value.credential_id,
            delegation_id: value.delegation_id,
            represented_user_id: value.represented_user_id,
            operation_id: value.operation_id,
            capability_key: value.capability_key,
            required_permission: value.required_permission,
            access_mode: value.access_mode,
            result: value.result,
            reason_code: value.reason_code,
            http_status: value.http_status,
            row_count: value.row_count,
            response_bytes: value.response_bytes,
            tenant_epoch: value.tenant_epoch.map(|epoch| epoch.to_string()),
            account_authorization_version: value.account_authorization_version,
            user_authorization_version: value.user_authorization_version,
            delegation_version: value.delegation_version,
            started_at: value.started_at,
            completed_at: value.completed_at,
        }
    }
}
