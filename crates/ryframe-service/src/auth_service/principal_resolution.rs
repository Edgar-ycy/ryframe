use async_trait::async_trait;
use ryframe_auth::{PrincipalResolver, RequestPrincipal, jwt::Claims};
use ryframe_core::Repository;
use ryframe_db::{ReadConsistency, entities::role};
use ryframe_kernel::{ActorContext, AppError, AppResult, DataScope, DataScopeContext};
use sea_orm::DatabaseConnection;

use crate::{AuthorizationSnapshot, AuthorizationVersions};

use super::{
    AuthService,
    identity::{AuthorizationProfile, ValidatedIdentity},
};

#[async_trait]
impl PrincipalResolver for AuthService {
    async fn resolve_principal(&self, claims: &Claims) -> AppResult<RequestPrincipal> {
        ryframe_core::validate_explicit_tenant(&claims.tenant_id)?;
        let user_id = claims
            .sub
            .parse::<i64>()
            .map_err(|_| AppError::Authentication("令牌中的用户ID无效".into()))?;

        let lookup = self
            .authorization_cache
            .lookup_snapshot(&claims.tenant_id, user_id)
            .await?;
        validate_mirrored_user_version(claims, lookup.user_authorization_version)?;
        if let Some(snapshot) = lookup.snapshot {
            return principal_from_snapshot(claims, user_id, snapshot);
        }

        for _ in 0..2 {
            let snapshot = self.rebuild_authorization_snapshot(claims).await?;
            if !self.authorization_cache.is_enabled()
                || self.authorization_cache.store_snapshot(&snapshot).await?
            {
                return Ok(snapshot.principal);
            }

            // 数据库读取期间发生了授权变更时，写脚本会拒绝旧版本；立即重读一次新快照。
            let lookup = self
                .authorization_cache
                .lookup_snapshot(&claims.tenant_id, user_id)
                .await?;
            validate_mirrored_user_version(claims, lookup.user_authorization_version)?;
            if let Some(snapshot) = lookup.snapshot {
                return principal_from_snapshot(claims, user_id, snapshot);
            }
        }

        Err(AppError::Authentication(
            "授权状态正在更新，请重新发起请求".into(),
        ))
    }
}

impl AuthService {
    async fn rebuild_authorization_snapshot(
        &self,
        claims: &Claims,
    ) -> AppResult<AuthorizationSnapshot> {
        let selected = self.db.select_read(ReadConsistency::Strong);
        let identity = self
            .validate_token_identity_on(&selected.connection, claims)
            .await?;
        let authorization = self
            .load_authorization_profile_on(
                &selected.connection,
                &identity.user.tenant_id,
                identity.user.id,
            )
            .await?;
        let data_scope = self
            .resolve_data_scope_on(
                &selected.connection,
                &identity.user.tenant_id,
                identity.user.id,
                identity.user.dept_id,
                &authorization.roles,
            )
            .await?;

        Ok(build_authorization_snapshot(
            &identity,
            authorization,
            data_scope,
        ))
    }

    async fn resolve_data_scope_on(
        &self,
        db: &DatabaseConnection,
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
                .find_by_id(db, tenant_id, dept_id)
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
            .find_roles_dept_ids(db, tenant_id, &custom_role_ids)
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
                            .find_child_dept_ids(db, tenant_id, dept_id)
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

fn build_authorization_snapshot(
    identity: &ValidatedIdentity,
    authorization: AuthorizationProfile,
    data_scope: DataScopeContext,
) -> AuthorizationSnapshot {
    let user = &identity.user;
    let is_super_admin = authorization.roles.iter().any(|role| role.is_super == 1);
    let role_ids = authorization.roles.iter().map(|role| role.id).collect();
    let roles = authorization
        .roles
        .iter()
        .map(|role| role.code.clone())
        .collect();

    AuthorizationSnapshot {
        versions: AuthorizationVersions {
            tenant_authorization_epoch: identity.tenant.authorization_epoch,
            user_authorization_version: user.authorization_version,
        },
        tenant_session_version: identity.tenant.session_version,
        principal: RequestPrincipal {
            actor: ActorContext {
                user_id: user.id,
                tenant_id: user.tenant_id.clone(),
                username: user.username.clone(),
                dept_id: user.dept_id,
                dept_path: data_scope.ancestors.clone(),
                data_scope: data_scope.scope,
                custom_dept_ids: data_scope.custom_dept_ids,
                include_self: data_scope.include_self,
                is_super_admin,
            },
            preferred_locale: user.preferred_locale.clone(),
            roles,
            role_ids,
            permissions: authorization.permissions,
            tenant_request_limit_per_minute: identity.tenant.max_requests_per_min.max(1) as u32,
        },
    }
}

fn validate_mirrored_user_version(
    claims: &Claims,
    user_authorization_version: Option<i32>,
) -> AppResult<()> {
    if user_authorization_version
        .is_some_and(|version| version != claims.user_authorization_version)
    {
        return Err(AppError::Authentication(
            "用户权限已变更，请重新登录".into(),
        ));
    }
    Ok(())
}

fn principal_from_snapshot(
    claims: &Claims,
    expected_user_id: i64,
    snapshot: AuthorizationSnapshot,
) -> AppResult<RequestPrincipal> {
    if snapshot.principal.actor.tenant_id != claims.tenant_id
        || snapshot.principal.actor.user_id != expected_user_id
        || snapshot.versions.user_authorization_version != claims.user_authorization_version
    {
        return Err(AppError::Authentication("授权快照身份不匹配".into()));
    }
    if snapshot.tenant_session_version != claims.tenant_session_version {
        return Err(AppError::Authentication(
            "租户会话已失效，请重新登录".into(),
        ));
    }
    Ok(snapshot.principal)
}
