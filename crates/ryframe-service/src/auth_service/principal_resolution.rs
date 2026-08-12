use async_trait::async_trait;
use ryframe_auth::{PrincipalResolver, RequestPrincipal, jwt::Claims};
use ryframe_db::ReadConsistency;
use ryframe_kernel::{ActorContext, AppError, AppResult};

use crate::{AuthorizationSnapshot, AuthorizationVersions, ResolvedAuthorization};

use super::{AuthService, identity::ValidatedIdentity};

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
            .authorization_resolver
            .resolve(
                &selected.connection,
                &identity.user.tenant_id,
                &identity.user,
            )
            .await?;

        Ok(build_authorization_snapshot(&identity, authorization))
    }
}

fn build_authorization_snapshot(
    identity: &ValidatedIdentity,
    authorization: ResolvedAuthorization,
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
                dept_path: authorization.data_scope.ancestors.clone(),
                data_scope: authorization.data_scope.scope,
                custom_dept_ids: authorization.data_scope.custom_dept_ids,
                include_self: authorization.data_scope.include_self,
                is_super_admin,
            },
            tenant_authorization_epoch: identity.tenant.authorization_epoch,
            preferred_locale: user.preferred_locale.clone(),
            roles,
            role_ids,
            permissions: authorization.permission_codes,
            // 租户写服务禁止负数；若历史数据损坏为负数，则按最严格的单次额度降级，不能意外放开限流。
            tenant_request_limit_per_minute: u32::try_from(identity.tenant.max_requests_per_min)
                .unwrap_or(1),
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
    mut snapshot: AuthorizationSnapshot,
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
    // 兼容升级前已写入 Redis、尚未过期且不含该展示字段的授权快照。
    snapshot.principal.tenant_authorization_epoch = snapshot.versions.tenant_authorization_epoch;
    Ok(snapshot.principal)
}
