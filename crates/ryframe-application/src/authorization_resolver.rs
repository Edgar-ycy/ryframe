use std::sync::Arc;

use ryframe_kernel::{AppResult, DataScope, DataScopeContext};

use crate::{IdentityAuthorizationReadPort, IdentityRoleRecord, IdentityUserRecord};

/// 从控制库事实计算得到的最终授权结果。
pub(crate) struct ResolvedAuthorization {
    pub roles: Vec<IdentityRoleRecord>,
    pub permission_codes: Vec<String>,
    pub data_scope: DataScopeContext,
}

impl ResolvedAuthorization {
    /// 超级管理员身份只由角色表的显式标记决定，禁止从角色编码或名称推断。
    pub(crate) fn is_super_admin(&self) -> bool {
        self.roles.iter().any(|role| role.is_super)
    }
}

pub(crate) struct AuthorizationResolver {
    persistence: Arc<dyn IdentityAuthorizationReadPort>,
}

impl AuthorizationResolver {
    pub fn new(persistence: Arc<dyn IdentityAuthorizationReadPort>) -> Self {
        Self { persistence }
    }

    pub async fn tenant(&self, tenant_id: &str) -> AppResult<Option<crate::IdentityTenantRecord>> {
        self.persistence.tenant(tenant_id).await
    }

    pub async fn user_by_id(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<Option<IdentityUserRecord>> {
        self.persistence.user_by_id(tenant_id, user_id).await
    }

    pub async fn user_by_username(
        &self,
        tenant_id: &str,
        username: &str,
    ) -> AppResult<Option<IdentityUserRecord>> {
        self.persistence.user_by_username(tenant_id, username).await
    }

    pub async fn department_name(
        &self,
        tenant_id: &str,
        dept_id: i64,
    ) -> AppResult<Option<String>> {
        self.persistence.department_name(tenant_id, dept_id).await
    }

    pub async fn resolve(
        &self,
        tenant_id: &str,
        user: &IdentityUserRecord,
    ) -> AppResult<ResolvedAuthorization> {
        let mut roles = self.persistence.roles(tenant_id, user.id).await?;
        roles.sort_unstable_by_key(|role| role.id);
        let is_super_admin = roles.iter().any(|role| role.is_super);
        let permission_codes = if is_super_admin {
            vec!["*:*:*".to_owned()]
        } else {
            let role_ids = roles.iter().map(|role| role.id).collect::<Vec<_>>();
            let mut codes = self
                .persistence
                .permission_codes(tenant_id, &role_ids)
                .await?;
            codes.sort_unstable();
            codes.dedup();
            codes
        };
        let data_scope = self.resolve_data_scope(tenant_id, user, &roles).await?;
        Ok(ResolvedAuthorization {
            roles,
            permission_codes,
            data_scope,
        })
    }

    async fn resolve_data_scope(
        &self,
        tenant_id: &str,
        user: &IdentityUserRecord,
        roles: &[IdentityRoleRecord],
    ) -> AppResult<DataScopeContext> {
        if roles.iter().any(|role| role.is_super) {
            return Ok(DataScopeContext::super_admin(user.id));
        }

        let ancestors = match user.dept_id {
            Some(dept_id) => {
                self.persistence
                    .department_ancestors(tenant_id, dept_id)
                    .await?
            }
            None => None,
        };
        let custom_role_ids = roles
            .iter()
            .filter(|role| DataScope::from_db_value(&role.data_scope) == DataScope::Custom)
            .map(|role| role.id)
            .collect::<Vec<_>>();
        let custom_dept_ids = self
            .persistence
            .role_department_ids(tenant_id, &custom_role_ids)
            .await?;
        let mut scopes = Vec::with_capacity(roles.len());

        for role in roles {
            let scope = DataScope::from_db_value(&role.data_scope);
            let scope_dept_ids = match scope {
                DataScope::Custom => custom_dept_ids.clone(),
                DataScope::Dept => user.dept_id.into_iter().collect(),
                DataScope::DeptAndChildren => match user.dept_id {
                    Some(dept_id) => {
                        self.persistence
                            .child_department_ids(tenant_id, dept_id)
                            .await?
                    }
                    None => Vec::new(),
                },
                DataScope::All | DataScope::SelfOnly => Vec::new(),
            };
            scopes.push(DataScopeContext {
                scope,
                user_id: user.id,
                dept_id: user.dept_id,
                ancestors: ancestors.clone(),
                custom_dept_ids: scope_dept_ids,
                include_self: false,
            });
        }

        if scopes.is_empty() {
            return Ok(DataScopeContext {
                scope: DataScope::SelfOnly,
                user_id: user.id,
                dept_id: user.dept_id,
                ancestors,
                custom_dept_ids: Vec::new(),
                include_self: true,
            });
        }
        Ok(DataScopeContext::merge(scopes))
    }
}

#[cfg(test)]
mod tests {
    use ryframe_kernel::DataScopeContext;

    use super::{IdentityRoleRecord, ResolvedAuthorization};

    fn role(code: &str, is_super: bool) -> IdentityRoleRecord {
        IdentityRoleRecord {
            id: 1,
            code: code.into(),
            is_super,
            data_scope: "5".into(),
        }
    }

    fn authorization(roles: Vec<IdentityRoleRecord>) -> ResolvedAuthorization {
        ResolvedAuthorization {
            roles,
            permission_codes: Vec::new(),
            data_scope: DataScopeContext::super_admin(1),
        }
    }

    #[test]
    fn super_admin_uses_explicit_role_marker_instead_of_code() {
        assert!(!authorization(vec![role("admin", false)]).is_super_admin());
        assert!(authorization(vec![role("ordinary", true)]).is_super_admin());
    }
}
