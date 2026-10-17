use async_trait::async_trait;
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_kernel::{ActorContext, AppError, AppResult};

use crate::{
    AuthorizationSnapshot, AuthorizationVersions, PrincipalResolver, ResolvedAuthorization,
};

use super::{AuthService, identity::ValidatedIdentity};

#[async_trait]
impl PrincipalResolver for AuthService {
    async fn resolve_principal(&self, claims: &Claims) -> AppResult<RequestPrincipal> {
        crate::enforce_tenant_scope(&claims.tenant_id)?;
        let user_id = claims
            .sub
            .parse::<i64>()
            .map_err(|_| AppError::Authentication("令牌中的用户ID无效".into()))?;

        let lookup = self
            .authorization_cache
            .lookup_snapshot(&claims.tenant_id, user_id)
            .await?;
        let current = mirrored_user_matches(claims, lookup.user_authorization_version)?;
        if current && let Some(snapshot) = lookup.snapshot {
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
            let current = mirrored_user_matches(claims, lookup.user_authorization_version)?;
            if current && let Some(snapshot) = lookup.snapshot {
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
        let identity = self.validate_token_identity(claims).await?;
        let authorization = self
            .authorization_resolver
            .resolve(&identity.user.tenant_id, &identity.user)
            .await?;

        Ok(build_authorization_snapshot(&identity, authorization))
    }
}

fn build_authorization_snapshot(
    identity: &ValidatedIdentity,
    authorization: ResolvedAuthorization,
) -> AuthorizationSnapshot {
    let user = &identity.user;
    let is_super_admin = authorization.is_super_admin();
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
            password_change_required: user.password_change_required,
            user_authorization_version: user.authorization_version,
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

fn mirrored_user_matches(
    claims: &Claims,
    user_authorization_version: Option<i32>,
) -> AppResult<bool> {
    if user_authorization_version.is_some_and(|version| version > claims.user_authorization_version)
    {
        return Err(AppError::Authentication(
            "用户权限已变更，请重新登录".into(),
        ));
    }
    // 镜像落后于已签名令牌时，必须先校验主库并重建，不能复用旧快照。
    // store_snapshot 的原子版本检查只向前推进，竞争中的更新不会被降级。
    Ok(user_authorization_version
        .is_none_or(|version| version == claims.user_authorization_version))
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
    snapshot.validate_principal_versions()?;
    Ok(snapshot.principal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> AuthorizationSnapshot {
        let principal = RequestPrincipal {
            actor: ActorContext {
                user_id: 2,
                tenant_id: "system".into(),
                username: "user".into(),
                dept_id: None,
                dept_path: None,
                data_scope: ryframe_kernel::DataScope::SelfOnly,
                custom_dept_ids: Vec::new(),
                include_self: true,
                is_super_admin: false,
            },
            password_change_required: false,
            user_authorization_version: 3,
            tenant_authorization_epoch: 7,
            preferred_locale: None,
            roles: Vec::new(),
            role_ids: Vec::new(),
            permissions: Vec::new(),
            tenant_request_limit_per_minute: 60,
        };
        AuthorizationSnapshot {
            versions: AuthorizationVersions {
                tenant_authorization_epoch: 7,
                user_authorization_version: 3,
            },
            tenant_session_version: 1,
            principal,
        }
    }

    #[test]
    fn snapshot_versions_remain_bound_to_the_authenticated_principal() {
        let principal = principal_from_snapshot(&claims(3), 2, snapshot()).unwrap();
        assert_eq!(principal.user_authorization_version, 3);
        assert_eq!(principal.tenant_authorization_epoch, 7);
        for (user, tenant) in [(2, 7), (4, 7), (3, 6), (3, 8)] {
            let mut stale = snapshot();
            stale.principal.user_authorization_version = user;
            stale.principal.tenant_authorization_epoch = tenant;
            assert!(
                matches!(
                    principal_from_snapshot(&claims(3), 2, stale),
                    Err(AppError::Authentication(_))
                ),
                "不能把包装版本覆盖到旧主体"
            );
        }
    }

    #[tokio::test]
    async fn invalid_snapshot_is_rejected_before_any_cache_write() {
        let cache =
            crate::AuthorizationCache::new(None, None, crate::CacheAvailabilityPolicy::Optional);
        let mut invalid = snapshot();
        invalid.principal.tenant_authorization_epoch = 6;
        assert!(matches!(
            cache.store_snapshot(&invalid).await,
            Err(AppError::Authentication(_))
        ));
        assert!(!cache.store_snapshot(&snapshot()).await.unwrap());
    }

    #[test]
    fn serialized_principal_requires_both_exact_version_fields() {
        let document = serde_json::to_value(snapshot().principal).unwrap();
        assert!(serde_json::from_value::<RequestPrincipal>(document.clone()).is_ok());
        for field in ["user_authorization_version", "tenant_authorization_epoch"] {
            let mut missing = document.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<RequestPrincipal>(missing).is_err());
            for value in [
                serde_json::json!("3"),
                serde_json::json!(true),
                serde_json::Value::Null,
            ] {
                let mut invalid = document.clone();
                invalid[field] = value;
                assert!(serde_json::from_value::<RequestPrincipal>(invalid).is_err());
            }
        }
    }

    fn claims(version: i32) -> Claims {
        Claims {
            sub: "2".into(),
            tenant_id: "system".into(),
            tenant_session_version: 1,
            user_authorization_version: version,
            username: "user".into(),
            token_type: "access".into(),
            sid: "session".into(),
            jti: "token".into(),
            iat: 0,
            exp: usize::MAX,
        }
    }

    #[test]
    fn lagging_mirror_requires_authoritative_rebuild() {
        assert!(!mirrored_user_matches(&claims(3), Some(2)).unwrap());
        assert!(!mirrored_user_matches(&claims(3), Some(1)).unwrap());
    }

    #[test]
    fn current_or_missing_mirror_can_use_validated_snapshot() {
        assert!(mirrored_user_matches(&claims(3), Some(3)).unwrap());
        assert!(mirrored_user_matches(&claims(3), None).unwrap());
    }

    #[test]
    fn newer_mirror_keeps_old_token_rejected() {
        assert!(matches!(
            mirrored_user_matches(&claims(2), Some(3)),
            Err(AppError::Authentication(_))
        ));
    }
}
