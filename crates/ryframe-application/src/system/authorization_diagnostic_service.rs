use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use ryframe_kernel::{ActorContext, AppError, AppResult, DataScope};
use serde::Serialize;

use crate::{
    AuthorizationCache,
    ports::{
        auth::IdentityRoleRecord,
        authorization::{
            AuthorizationDiagnosticReadPort, DiagnosticMenuRecord, DiagnosticPermissionRecord,
        },
    },
};

use super::UserService;

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
pub struct AuthorizationDiagnosticTenantVo {
    pub tenant_id: String,
    pub name: String,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub available: bool,
}

#[derive(Debug, Serialize)]
pub struct AuthorizationDiagnosticRoleVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub status: String,
    pub data_scope: String,
    pub is_super: bool,
    pub participates: bool,
}

#[derive(Debug, Serialize)]
pub struct AuthorizationDiagnosticPermissionVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub source_roles: Vec<String>,
    pub effective: bool,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
pub struct AuthorizationDiagnosticDataScopeVo {
    pub scope: String,
    pub include_self: bool,
    pub department_path: Vec<AuthorizationDiagnosticDepartmentVo>,
    pub custom_departments: Vec<AuthorizationDiagnosticDepartmentVo>,
    pub sources: Vec<AuthorizationDiagnosticDataScopeSourceVo>,
}

#[derive(Debug, Serialize)]
pub struct AuthorizationDiagnosticDepartmentVo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct AuthorizationDiagnosticDataScopeSourceVo {
    pub role_code: String,
    pub scope: String,
}

#[derive(Debug, Serialize)]
pub struct AuthorizationDiagnosticVersionVo {
    pub tenant_authorization_epoch: i32,
    pub user_authorization_version: i32,
    pub cache_status: String,
    pub cached_tenant_authorization_epoch: Option<i32>,
    pub cached_user_authorization_version: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct AuthorizationDiagnosticRefreshVo {
    pub websocket_notification_available: bool,
    pub response_header_epoch_fallback_available: bool,
    pub websocket_online_state_asserted: bool,
}

#[derive(Default)]
struct PermissionSource {
    permission: Option<DiagnosticPermissionRecord>,
    roles: BTreeSet<String>,
}

pub struct AuthorizationDiagnosticService {
    persistence: Arc<dyn AuthorizationDiagnosticReadPort>,
    user: Arc<UserService>,
    authorization_cache: AuthorizationCache,
    websocket_notification_available: bool,
}

impl AuthorizationDiagnosticService {
    pub fn new(
        persistence: Arc<dyn AuthorizationDiagnosticReadPort>,
        user: Arc<UserService>,
        authorization_cache: AuthorizationCache,
        websocket_notification_available: bool,
    ) -> Self {
        Self {
            persistence,
            user,
            authorization_cache,
            websocket_notification_available,
        }
    }

    pub async fn diagnose(
        &self,
        actor: &ActorContext,
        user_id: i64,
    ) -> AppResult<AuthorizationDiagnosticVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        if user_id <= 0 {
            return Err(AppError::Validation("用户ID必须大于零".into()));
        }
        let target_tenant = self
            .persistence
            .user_tenant_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        if target_tenant != tenant_id {
            return Err(AppError::Authorization("禁止跨租户诊断用户授权".into()));
        }
        if self.user.find_by_id(actor, user_id).await?.is_none() {
            return Err(AppError::NotFound("用户不存在或不在当前数据范围内".into()));
        }

        let authorization = self
            .user
            .calculate_current_authorization(tenant_id, user_id)
            .await?;
        let calculated_at = self.persistence.database_now().await?;
        let tenant_available = authorization.tenant.is_available(calculated_at);
        let user_enabled = authorization.user.is_enabled();
        let final_access_enabled = tenant_available && user_enabled;
        let enabled_role_ids = authorization
            .roles
            .iter()
            .map(|role| role.id)
            .collect::<HashSet<_>>();
        let mut assigned_roles = self.persistence.assigned_roles(tenant_id, user_id).await?;
        assigned_roles.sort_unstable_by_key(|role| role.id);

        let roles = assigned_roles
            .into_iter()
            .map(|role| AuthorizationDiagnosticRoleVo {
                id: role.id.to_string(),
                name: role.name,
                code: role.code,
                status: role.status,
                data_scope: data_scope_key(&DataScope::from_db_value(&role.data_scope)).to_owned(),
                is_super: role.is_super,
                participates: final_access_enabled && enabled_role_ids.contains(&role.id),
            })
            .collect::<Vec<_>>();

        let all_permissions = self.persistence.permissions(tenant_id).await?;
        let permission_by_id = all_permissions
            .iter()
            .map(|permission| (permission.id, permission))
            .collect::<HashMap<_, _>>();
        let active_permissions = all_permissions
            .iter()
            .filter(|permission| permission.status == "1")
            .cloned()
            .collect::<Vec<_>>();
        let permission_sources = self
            .permission_sources(tenant_id, &authorization.roles, &active_permissions)
            .await?;
        let permissions = permission_sources
            .into_values()
            .filter_map(|source| {
                source
                    .permission
                    .map(|permission| AuthorizationDiagnosticPermissionVo {
                        id: permission.id.to_string(),
                        name: permission.name,
                        code: permission.code,
                        source_roles: source.roles.into_iter().collect(),
                        effective: final_access_enabled,
                    })
            })
            .collect::<Vec<_>>();

        let all_menus = self.persistence.menus(tenant_id).await?;
        let accessible_ids = if final_access_enabled {
            self.persistence
                .accessible_menu_ids(tenant_id, &authorization.permission_codes)
                .await?
                .into_iter()
                .collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };
        let mut invalid_menu_permission = false;
        let menus = all_menus
            .into_iter()
            .filter(|menu| !menu.is_button())
            .map(|menu| {
                let permission = menu
                    .perm_id
                    .and_then(|permission_id| permission_by_id.get(&permission_id).copied());
                if menu.perm_id.is_some() && permission.is_none() {
                    invalid_menu_permission = true;
                }
                let accessible = accessible_ids.contains(&menu.id);
                let inaccessible_reason = menu_inaccessible_reason(
                    &menu,
                    permission,
                    tenant_available,
                    user_enabled,
                    accessible,
                );
                AuthorizationDiagnosticMenuVo {
                    id: menu.id.to_string(),
                    parent_id: menu.parent_id.map(|id| id.to_string()),
                    name: menu.name,
                    route_key: menu.route_key,
                    permission_code: permission.map(|permission| permission.code.clone()),
                    status: menu.status,
                    configured_visible: menu.visible,
                    accessible,
                    visible_in_navigation: accessible && menu.visible,
                    inaccessible_reason,
                }
            })
            .collect::<Vec<_>>();

        let department_path = self
            .department_path(
                tenant_id,
                authorization.actor.dept_path.as_deref(),
                authorization.actor.dept_id,
            )
            .await?;
        let custom_departments = self
            .departments(tenant_id, &authorization.actor.custom_dept_ids)
            .await?;
        let data_scope_sources = authorization
            .roles
            .iter()
            .map(|role| AuthorizationDiagnosticDataScopeSourceVo {
                role_code: role.code.clone(),
                scope: data_scope_key(&DataScope::from_db_value(&role.data_scope)).to_owned(),
            })
            .collect::<Vec<_>>();

        let (versions, cache_warning) = self
            .diagnose_cache(
                tenant_id,
                user_id,
                authorization.tenant.authorization_epoch,
                authorization.user.authorization_version,
            )
            .await;
        let mut warnings = Vec::new();
        if !user_enabled {
            warnings.push("user_disabled".to_owned());
        }
        if authorization.tenant.status != "enabled" {
            warnings.push("tenant_disabled".to_owned());
        } else if authorization
            .tenant
            .expire_at
            .is_some_and(|expire_at| expire_at <= calculated_at)
        {
            warnings.push("tenant_expired".to_owned());
        }
        if authorization.roles.is_empty() {
            warnings.push("no_enabled_roles".to_owned());
        }
        if let Some(warning) = cache_warning {
            warnings.push(warning);
        }
        if invalid_menu_permission {
            warnings.push("invalid_menu_permission_reference".to_owned());
        }

        let dept_name = department_path.last().map(|dept| dept.name.clone());
        Ok(AuthorizationDiagnosticVo {
            calculated_at,
            user: AuthorizationDiagnosticUserVo {
                id: authorization.user.id.to_string(),
                username: authorization.user.username,
                nickname: authorization.user.nickname,
                status: authorization.user.status,
                dept_id: authorization.user.dept_id.map(|id| id.to_string()),
                dept_name,
                enabled: user_enabled,
                final_access_enabled,
            },
            tenant: AuthorizationDiagnosticTenantVo {
                tenant_id: authorization.tenant.tenant_id,
                name: authorization.tenant.name,
                status: authorization.tenant.status,
                expire_at: authorization.tenant.expire_at,
                available: tenant_available,
            },
            roles,
            permissions,
            menus,
            data_scope: AuthorizationDiagnosticDataScopeVo {
                scope: data_scope_key(&authorization.actor.data_scope).to_owned(),
                include_self: authorization.actor.include_self,
                department_path,
                custom_departments,
                sources: data_scope_sources,
            },
            versions,
            dynamic_refresh: AuthorizationDiagnosticRefreshVo {
                websocket_notification_available: self.websocket_notification_available,
                response_header_epoch_fallback_available: true,
                websocket_online_state_asserted: false,
            },
            warnings,
        })
    }

    async fn permission_sources(
        &self,
        tenant_id: &str,
        roles: &[IdentityRoleRecord],
        active_permissions: &[DiagnosticPermissionRecord],
    ) -> AppResult<BTreeMap<String, PermissionSource>> {
        let mut sources = BTreeMap::<String, PermissionSource>::new();
        let super_roles = roles
            .iter()
            .filter(|role| role.is_super)
            .map(|role| role.code.clone())
            .collect::<BTreeSet<_>>();
        if !super_roles.is_empty() {
            for permission in active_permissions {
                sources.insert(
                    permission.code.clone(),
                    PermissionSource {
                        permission: Some(permission.clone()),
                        roles: super_roles.clone(),
                    },
                );
            }
            return Ok(sources);
        }

        for role in roles {
            for permission in self
                .persistence
                .role_permissions(tenant_id, role.id)
                .await?
            {
                let source = sources.entry(permission.code.clone()).or_default();
                source.permission = Some(permission);
                source.roles.insert(role.code.clone());
            }
        }
        Ok(sources)
    }

    async fn department_path(
        &self,
        tenant_id: &str,
        ancestors: Option<&str>,
        dept_id: Option<i64>,
    ) -> AppResult<Vec<AuthorizationDiagnosticDepartmentVo>> {
        let mut ids = ancestors
            .into_iter()
            .flat_map(|path| path.split(','))
            .filter_map(|id| id.parse::<i64>().ok())
            .filter(|id| *id > 0)
            .collect::<Vec<_>>();
        if let Some(dept_id) = dept_id {
            ids.push(dept_id);
        }
        let mut departments = self
            .persistence
            .departments(tenant_id, &ids)
            .await?
            .into_iter()
            .map(|dept| (dept.id, dept.name))
            .collect::<HashMap<_, _>>();
        Ok(ids
            .into_iter()
            .filter_map(|id| {
                departments
                    .remove(&id)
                    .map(|name| AuthorizationDiagnosticDepartmentVo {
                        id: id.to_string(),
                        name,
                    })
            })
            .collect())
    }

    async fn departments(
        &self,
        tenant_id: &str,
        ids: &[i64],
    ) -> AppResult<Vec<AuthorizationDiagnosticDepartmentVo>> {
        self.persistence
            .departments(tenant_id, ids)
            .await
            .map(|departments| {
                departments
                    .into_iter()
                    .map(|dept| AuthorizationDiagnosticDepartmentVo {
                        id: dept.id.to_string(),
                        name: dept.name,
                    })
                    .collect()
            })
    }

    async fn diagnose_cache(
        &self,
        tenant_id: &str,
        user_id: i64,
        tenant_epoch: i32,
        user_version: i32,
    ) -> (AuthorizationDiagnosticVersionVo, Option<String>) {
        let inspection = self
            .authorization_cache
            .inspect_snapshot(tenant_id, user_id)
            .await;
        let (status, cached_tenant_epoch, cached_user_version) = match inspection {
            Ok(Some(lookup)) => {
                let stale = lookup
                    .tenant_authorization_epoch
                    .is_some_and(|version| version != tenant_epoch)
                    || lookup
                        .user_authorization_version
                        .is_some_and(|version| version != user_version);
                let status = if stale {
                    "stale"
                } else if lookup.snapshot.is_some() {
                    "current"
                } else {
                    "missing"
                };
                (
                    status,
                    lookup.tenant_authorization_epoch,
                    lookup.user_authorization_version,
                )
            }
            Ok(None) | Err(_) => ("unavailable", None, None),
        };
        let warning = (status != "current").then(|| format!("authorization_cache_{status}"));
        (
            AuthorizationDiagnosticVersionVo {
                tenant_authorization_epoch: tenant_epoch,
                user_authorization_version: user_version,
                cache_status: status.to_owned(),
                cached_tenant_authorization_epoch: cached_tenant_epoch,
                cached_user_authorization_version: cached_user_version,
            },
            warning,
        )
    }
}

fn data_scope_key(scope: &DataScope) -> &'static str {
    match scope {
        DataScope::All => "all",
        DataScope::Custom => "custom",
        DataScope::Dept => "department",
        DataScope::DeptAndChildren => "department_and_children",
        DataScope::SelfOnly => "self_only",
    }
}

fn menu_inaccessible_reason(
    menu: &DiagnosticMenuRecord,
    permission: Option<&DiagnosticPermissionRecord>,
    tenant_available: bool,
    user_enabled: bool,
    accessible: bool,
) -> Option<String> {
    if accessible {
        return None;
    }
    let reason = if !tenant_available {
        "tenant_unavailable"
    } else if !user_enabled {
        "user_disabled"
    } else if !menu.is_enabled() {
        "menu_disabled"
    } else if menu.is_dir() {
        "no_accessible_child"
    } else if menu.perm_id.is_none() {
        "permission_missing"
    } else if permission.is_none() {
        "invalid_permission_reference"
    } else if permission.is_some_and(|permission| permission.status != "1") {
        "permission_disabled"
    } else {
        "permission_not_granted"
    };
    Some(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticMenuRecord, menu_inaccessible_reason};

    fn menu(status: &str, menu_type: &str, perm_id: Option<i64>) -> DiagnosticMenuRecord {
        DiagnosticMenuRecord {
            id: 1,
            parent_id: None,
            name: "测试菜单".to_owned(),
            route_key: Some("test".to_owned()),
            perm_id,
            menu_type: menu_type.to_owned(),
            status: status.to_owned(),
            visible: true,
        }
    }

    #[test]
    fn inaccessible_reason_prefers_account_state_before_menu_configuration() {
        let disabled_menu = menu("0", "C", None);

        assert_eq!(
            menu_inaccessible_reason(&disabled_menu, None, false, false, false).as_deref(),
            Some("tenant_unavailable")
        );
        assert_eq!(
            menu_inaccessible_reason(&disabled_menu, None, true, false, false).as_deref(),
            Some("user_disabled")
        );
        assert_eq!(
            menu_inaccessible_reason(&disabled_menu, None, true, true, false).as_deref(),
            Some("menu_disabled")
        );
    }

    #[test]
    fn inaccessible_reason_distinguishes_directory_and_permission_configuration() {
        assert_eq!(
            menu_inaccessible_reason(&menu("1", "M", None), None, true, true, false).as_deref(),
            Some("no_accessible_child")
        );
        assert_eq!(
            menu_inaccessible_reason(&menu("1", "C", None), None, true, true, false).as_deref(),
            Some("permission_missing")
        );
        assert_eq!(
            menu_inaccessible_reason(&menu("1", "C", Some(9)), None, true, true, false).as_deref(),
            Some("invalid_permission_reference")
        );
    }
}
