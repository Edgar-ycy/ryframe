use async_trait::async_trait;
use ryframe_auth::{PrincipalResolver, RequestPrincipal, jwt::Claims};
use ryframe_common::{
    ActorContext, AppResult,
    annotations::data_scope::{DataScope, DataScopeContext},
};
use ryframe_core::Repository;
use ryframe_db::entities::{role, user};

use super::{AuthService, identity::AuthorizationProfile};

#[async_trait]
impl PrincipalResolver for AuthService {
    async fn resolve_principal(&self, claims: &Claims) -> AppResult<RequestPrincipal> {
        let identity = self.validate_token_identity(claims).await?;
        let authorization = self
            .load_authorization_profile(&identity.user.tenant_id, identity.user.id, true)
            .await?;
        let data_scope = self
            .resolve_data_scope(
                &identity.user.tenant_id,
                identity.user.id,
                identity.user.dept_id,
                &authorization.roles,
            )
            .await?;

        Ok(build_request_principal(
            claims,
            &identity.user,
            authorization,
            data_scope,
            identity.tenant.max_requests_per_min.max(1) as u32,
        ))
    }
}

impl AuthService {
    async fn resolve_data_scope(
        &self,
        tenant_id: &str,
        user_id: i64,
        dept_id: Option<i64>,
        roles: &[role::Model],
    ) -> AppResult<DataScopeContext> {
        if roles.iter().any(|role| role.is_super == 1) {
            return Ok(DataScopeContext::super_admin(user_id));
        }

        let ancestors = match dept_id {
            Some(dept_id) => self
                .dept_repo
                .find_by_id(self.db.write(), tenant_id, dept_id)
                .await?
                .map(|dept| dept.ancestors),
            None => None,
        };
        let custom_role_ids = roles
            .iter()
            .filter(|role| role.data_scope == role::Model::DATA_SCOPE_CUSTOM)
            .map(|role| role.id)
            .collect::<Vec<_>>();
        let custom_dept_ids = self
            .role_repo
            .find_roles_dept_ids(self.db.write(), tenant_id, &custom_role_ids)
            .await?;
        let mut scopes = Vec::with_capacity(roles.len());

        for role in roles {
            let scope = DataScope::from_db_value(&role.data_scope);
            let scope_dept_ids = match scope {
                DataScope::Custom => custom_dept_ids.clone(),
                DataScope::Dept => dept_id.into_iter().collect(),
                DataScope::DeptAndChildren => match dept_id {
                    Some(dept_id) => {
                        self.dept_repo
                            .find_child_dept_ids(self.db.write(), tenant_id, dept_id)
                            .await?
                    }
                    None => Vec::new(),
                },
                DataScope::All | DataScope::SelfOnly => Vec::new(),
            };
            scopes.push(DataScopeContext {
                scope,
                user_id,
                dept_id,
                ancestors: ancestors.clone(),
                custom_dept_ids: scope_dept_ids,
                include_self: false,
            });
        }

        if scopes.is_empty() {
            return Ok(DataScopeContext {
                scope: DataScope::SelfOnly,
                user_id,
                dept_id,
                ancestors,
                custom_dept_ids: Vec::new(),
                include_self: true,
            });
        }

        Ok(DataScopeContext::merge(scopes))
    }
}

fn build_request_principal(
    claims: &Claims,
    user: &user::Model,
    authorization: AuthorizationProfile,
    data_scope: DataScopeContext,
    tenant_request_limit_per_minute: u32,
) -> RequestPrincipal {
    let is_super_admin = authorization.roles.iter().any(|role| role.is_super == 1);
    let role_ids = authorization.roles.iter().map(|role| role.id).collect();
    let roles = authorization
        .roles
        .iter()
        .map(|role| role.code.clone())
        .collect();

    RequestPrincipal {
        actor: ActorContext {
            user_id: user.id,
            tenant_id: claims.tenant_id.clone(),
            username: claims.username.clone(),
            dept_id: user.dept_id,
            dept_path: data_scope.ancestors.clone(),
            data_scope: data_scope.scope,
            custom_dept_ids: data_scope.custom_dept_ids,
            include_self: data_scope.include_self,
            is_super_admin,
        },
        roles,
        role_ids,
        permissions: authorization.permissions,
        tenant_request_limit_per_minute,
    }
}
