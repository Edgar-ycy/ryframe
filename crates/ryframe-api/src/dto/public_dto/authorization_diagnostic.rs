use chrono::{DateTime, Utc};
use ryframe_service::system as service;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticVo {
    pub calculated_at: DateTime<Utc>,
    pub user: AuthorizationDiagnosticUserVo,
    pub tenant: AuthorizationDiagnosticTenantVo,
    pub roles: Vec<AuthorizationDiagnosticRoleVo>,
    pub permissions: Vec<AuthorizationDiagnosticPermissionVo>,
    pub menus: Vec<AuthorizationDiagnosticMenuVo>,
    pub data_scope: AuthorizationDiagnosticDataScopeVo,
    pub versions: AuthorizationDiagnosticVersionVo,
    pub dynamic_refresh: AuthorizationDiagnosticRefreshVo,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticUserVo {
    pub id: String,
    pub username: String,
    pub nickname: String,
    pub status: String,
    pub dept_id: Option<String>,
    pub dept_name: Option<String>,
    pub enabled: bool,
    pub final_access_enabled: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticTenantVo {
    pub tenant_id: String,
    pub name: String,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub available: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticRoleVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub status: String,
    pub data_scope: String,
    pub is_super: bool,
    pub participates: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticPermissionVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub source_roles: Vec<String>,
    pub effective: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticMenuVo {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub route_key: Option<String>,
    pub permission_code: Option<String>,
    pub status: String,
    pub configured_visible: bool,
    pub accessible: bool,
    pub visible_in_navigation: bool,
    pub inaccessible_reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticDataScopeVo {
    pub scope: String,
    pub include_self: bool,
    pub department_path: Vec<AuthorizationDiagnosticDepartmentVo>,
    pub custom_departments: Vec<AuthorizationDiagnosticDepartmentVo>,
    pub sources: Vec<AuthorizationDiagnosticDataScopeSourceVo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticDepartmentVo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticDataScopeSourceVo {
    pub role_code: String,
    pub scope: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticVersionVo {
    pub tenant_authorization_epoch: i32,
    pub user_authorization_version: i32,
    pub cache_status: String,
    pub cached_tenant_authorization_epoch: Option<i32>,
    pub cached_user_authorization_version: Option<i32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizationDiagnosticRefreshVo {
    pub websocket_notification_available: bool,
    pub response_header_epoch_fallback_available: bool,
    pub websocket_online_state_asserted: bool,
}

impl From<service::AuthorizationDiagnosticVo> for AuthorizationDiagnosticVo {
    fn from(value: service::AuthorizationDiagnosticVo) -> Self {
        Self {
            calculated_at: value.calculated_at,
            user: value.user.into(),
            tenant: value.tenant.into(),
            roles: value.roles.into_iter().map(Into::into).collect(),
            permissions: value.permissions.into_iter().map(Into::into).collect(),
            menus: value.menus.into_iter().map(Into::into).collect(),
            data_scope: value.data_scope.into(),
            versions: value.versions.into(),
            dynamic_refresh: value.dynamic_refresh.into(),
            warnings: value.warnings,
        }
    }
}

impl From<service::AuthorizationDiagnosticUserVo> for AuthorizationDiagnosticUserVo {
    fn from(value: service::AuthorizationDiagnosticUserVo) -> Self {
        Self {
            id: value.id,
            username: value.username,
            nickname: value.nickname,
            status: value.status,
            dept_id: value.dept_id,
            dept_name: value.dept_name,
            enabled: value.enabled,
            final_access_enabled: value.final_access_enabled,
        }
    }
}

impl From<service::AuthorizationDiagnosticTenantVo> for AuthorizationDiagnosticTenantVo {
    fn from(value: service::AuthorizationDiagnosticTenantVo) -> Self {
        Self {
            tenant_id: value.tenant_id,
            name: value.name,
            status: value.status,
            expire_at: value.expire_at,
            available: value.available,
        }
    }
}

impl From<service::AuthorizationDiagnosticRoleVo> for AuthorizationDiagnosticRoleVo {
    fn from(value: service::AuthorizationDiagnosticRoleVo) -> Self {
        Self {
            id: value.id,
            name: value.name,
            code: value.code,
            status: value.status,
            data_scope: value.data_scope,
            is_super: value.is_super,
            participates: value.participates,
        }
    }
}

impl From<service::AuthorizationDiagnosticPermissionVo> for AuthorizationDiagnosticPermissionVo {
    fn from(value: service::AuthorizationDiagnosticPermissionVo) -> Self {
        Self {
            id: value.id,
            name: value.name,
            code: value.code,
            source_roles: value.source_roles,
            effective: value.effective,
        }
    }
}

impl From<service::AuthorizationDiagnosticMenuVo> for AuthorizationDiagnosticMenuVo {
    fn from(value: service::AuthorizationDiagnosticMenuVo) -> Self {
        Self {
            id: value.id,
            parent_id: value.parent_id,
            name: value.name,
            route_key: value.route_key,
            permission_code: value.permission_code,
            status: value.status,
            configured_visible: value.configured_visible,
            accessible: value.accessible,
            visible_in_navigation: value.visible_in_navigation,
            inaccessible_reason: value.inaccessible_reason,
        }
    }
}

impl From<service::AuthorizationDiagnosticDataScopeVo> for AuthorizationDiagnosticDataScopeVo {
    fn from(value: service::AuthorizationDiagnosticDataScopeVo) -> Self {
        Self {
            scope: value.scope,
            include_self: value.include_self,
            department_path: value.department_path.into_iter().map(Into::into).collect(),
            custom_departments: value
                .custom_departments
                .into_iter()
                .map(Into::into)
                .collect(),
            sources: value.sources.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<service::AuthorizationDiagnosticDepartmentVo> for AuthorizationDiagnosticDepartmentVo {
    fn from(value: service::AuthorizationDiagnosticDepartmentVo) -> Self {
        Self {
            id: value.id,
            name: value.name,
        }
    }
}

impl From<service::AuthorizationDiagnosticDataScopeSourceVo>
    for AuthorizationDiagnosticDataScopeSourceVo
{
    fn from(value: service::AuthorizationDiagnosticDataScopeSourceVo) -> Self {
        Self {
            role_code: value.role_code,
            scope: value.scope,
        }
    }
}

impl From<service::AuthorizationDiagnosticVersionVo> for AuthorizationDiagnosticVersionVo {
    fn from(value: service::AuthorizationDiagnosticVersionVo) -> Self {
        Self {
            tenant_authorization_epoch: value.tenant_authorization_epoch,
            user_authorization_version: value.user_authorization_version,
            cache_status: value.cache_status,
            cached_tenant_authorization_epoch: value.cached_tenant_authorization_epoch,
            cached_user_authorization_version: value.cached_user_authorization_version,
        }
    }
}

impl From<service::AuthorizationDiagnosticRefreshVo> for AuthorizationDiagnosticRefreshVo {
    fn from(value: service::AuthorizationDiagnosticRefreshVo) -> Self {
        Self {
            websocket_notification_available: value.websocket_notification_available,
            response_header_epoch_fallback_available: value
                .response_header_epoch_fallback_available,
            websocket_online_state_asserted: value.websocket_online_state_asserted,
        }
    }
}
