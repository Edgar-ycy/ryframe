use std::{collections::HashMap, sync::Arc};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::RETRY_AFTER},
    middleware::{from_fn, from_fn_with_state},
    routing::get,
};
use ryframe_api::{
    auth_middleware::require_permission,
    middleware::rate_limit::{
        RateLimitState, api_rate_limit_middleware, rate_limit_middleware,
        user_rate_limit_middleware,
    },
    rate_limit::{HttpRateLimiter, RateLimitDecision},
    settings::RateLimitSettings,
};
use ryframe_auth::jwt::Claims;
use tower::ServiceExt;

struct FixedLimiter(Result<RateLimitDecision, String>);

#[async_trait::async_trait]
impl HttpRateLimiter for FixedLimiter {
    async fn acquire(&self, _: &str, _: u64, _: u32) -> Result<RateLimitDecision, String> {
        self.0.clone()
    }
}

fn settings(enabled: bool) -> RateLimitSettings {
    RateLimitSettings {
        enabled,
        capacity: 10,
        window_secs: 60,
        enable_user_rate_limit: true,
        user_window_secs: 60,
        user_capacity: 10,
        api_limits: HashMap::from([("GET".to_owned(), 10)]),
        api_window_secs: 60,
    }
}

fn claims() -> Claims {
    Claims {
        sub: "1".to_owned(),
        tenant_id: "system".to_owned(),
        tenant_session_version: 1,
        user_authorization_version: 1,
        username: "测试用户".to_owned(),
        token_type: "access".to_owned(),
        sid: "测试会话".to_owned(),
        jti: "测试令牌".to_owned(),
        iat: 1,
        exp: usize::MAX,
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("测试运行时应可用")
}

#[test]
fn rate_limit_responses_preserve_status_body_and_retry_header() {
    runtime().block_on(async {
        for kind in 0..3 {
            for (decision, expected, retry_after) in [
                (
                    Ok(RateLimitDecision {
                        allowed: true,
                        retry_after_secs: 0,
                    }),
                    StatusCode::OK,
                    None,
                ),
                (
                    Ok(RateLimitDecision {
                        allowed: false,
                        retry_after_secs: 0,
                    }),
                    StatusCode::TOO_MANY_REQUESTS,
                    Some("1"),
                ),
                (
                    Ok(RateLimitDecision {
                        allowed: false,
                        retry_after_secs: 12,
                    }),
                    StatusCode::TOO_MANY_REQUESTS,
                    Some("12"),
                ),
                (
                    Err("测试后端不可用".to_owned()),
                    StatusCode::SERVICE_UNAVAILABLE,
                    None,
                ),
            ] {
                let state = RateLimitState {
                    limiter: Arc::new(FixedLimiter(decision)),
                    config: Arc::new(settings(true)),
                };
                let app = limited_router(kind, state);
                let mut request = Request::builder().uri("/test").body(Body::empty()).unwrap();
                request.extensions_mut().insert(claims());
                let response = app.oneshot(request).await.unwrap();
                assert_eq!(response.status(), expected, "限流层 {kind}");
                assert_eq!(
                    response
                        .headers()
                        .get(RETRY_AFTER)
                        .map(|value| value.to_str().unwrap()),
                    retry_after
                );
                let body = to_bytes(response.into_body(), 1024).await.unwrap();
                let body = std::str::from_utf8(&body).unwrap();
                match expected {
                    StatusCode::OK => assert_eq!(body, "通过"),
                    StatusCode::TOO_MANY_REQUESTS => assert!(body.contains("请求过于频繁")),
                    _ => assert_eq!(body, "限流服务暂不可用，请稍后重试"),
                }
            }
        }
    });
}

fn limited_router(kind: usize, state: RateLimitState) -> Router {
    let app = Router::new().route("/test", get(|| async { "通过" }));
    match kind {
        0 => app.layer(from_fn_with_state(state, rate_limit_middleware)),
        1 => app.layer(from_fn_with_state(state, user_rate_limit_middleware)),
        _ => app.layer(from_fn_with_state(state, api_rate_limit_middleware)),
    }
}

#[test]
fn disabled_rate_limits_bypass_unavailable_backend() {
    runtime().block_on(async {
        for kind in 0..3 {
            let state = RateLimitState {
                limiter: Arc::new(FixedLimiter(Err("不应调用后端".to_owned()))),
                config: Arc::new(settings(false)),
            };
            let response = limited_router(kind, state)
                .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
    });
}

#[test]
fn permission_middleware_typed_error_preserves_unauthorized_response() {
    runtime().block_on(async {
        let app = Router::new()
            .route("/test", get(protected_handler))
            .layer(from_fn(require_permission("system:post:list")));
        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], 401);
        assert_eq!(body["error_key"], "authentication");
    });
}

async fn protected_handler() -> &'static str {
    panic!("未认证请求不得进入处理器")
}

mod first_password {
    use super::*;
    use axum::{
        extract::OriginalUri, http::Method, middleware::Next, response::Response, routing::any,
    };
    use ryframe_api::{
        auth_middleware::{AuthState, auth_middleware},
        session_security::{AccessRevocationStore, RefreshSessionControl, SessionRevocation},
    };
    use ryframe_application::PrincipalResolver;
    use ryframe_auth::{
        RequestPrincipal,
        jwt::{TokenIdentity, TokenSettings, encode_access},
    };
    use ryframe_kernel::{ActorContext, AppResult, DataScope};

    struct Identity(RequestPrincipal);

    #[async_trait::async_trait]
    impl PrincipalResolver for Identity {
        async fn resolve_principal(&self, _: &Claims) -> AppResult<RequestPrincipal> {
            Ok(self.0.clone())
        }
    }

    #[async_trait::async_trait]
    impl AccessRevocationStore for Identity {
        async fn is_revoked(&self, _: &str) -> AppResult<bool> {
            Ok(false)
        }
        async fn revoke(&self, _: &str, _: u64) -> AppResult<()> {
            unreachable!()
        }
    }

    #[async_trait::async_trait]
    impl RefreshSessionControl for Identity {
        async fn is_active_for_identity(&self, _: &str, _: &str, _: i64) -> AppResult<bool> {
            Ok(true)
        }
        async fn revoke(&self, _: &str) -> AppResult<bool> {
            unreachable!()
        }
        async fn revoke_for_tenant(&self, _: &str, _: &str) -> AppResult<bool> {
            unreachable!()
        }
        async fn revoke_for_user(&self, _: &str, _: i64, _: &str) -> AppResult<SessionRevocation> {
            unreachable!()
        }
        async fn session_sids_for_user(&self, _: &str, _: i64) -> AppResult<Vec<String>> {
            unreachable!()
        }
        async fn revoke_other_sessions_for_user(
            &self,
            _: &str,
            _: i64,
            _: &str,
            _: &[String],
        ) -> AppResult<u64> {
            unreachable!()
        }
    }

    fn identity(restricted: bool, permitted: bool) -> Identity {
        Identity(RequestPrincipal {
            actor: ActorContext {
                user_id: 2,
                tenant_id: "system".into(),
                username: "user".into(),
                dept_id: None,
                dept_path: None,
                data_scope: DataScope::SelfOnly,
                custom_dept_ids: vec![],
                include_self: true,
                data_scope_version: 1,
                is_super_admin: false,
            },
            password_change_required: restricted,
            user_authorization_version: 1,
            data_scope_version: 1,
            tenant_authorization_epoch: 1,
            preferred_locale: None,
            roles: vec![],
            role_ids: vec![],
            permissions: if permitted {
                vec!["system:post:list".into()]
            } else {
                vec![]
            },
            tenant_request_limit_per_minute: 100,
        })
    }

    async fn reached(request: axum::extract::Request) -> &'static str {
        assert!(
            request
                .extensions()
                .get::<ryframe_api::RequestPrincipal>()
                .is_some()
        );
        "已进入处理器"
    }

    fn router(restricted: bool, permitted: bool, nested: bool) -> (Router, String) {
        let settings =
            Arc::new(TokenSettings::new("isolated-auth-middleware-secret", "1h", "1h").unwrap());
        let (token, _) = encode_access(
            &TokenIdentity {
                user_id: 2,
                tenant_id: "system",
                tenant_session_version: 1,
                user_authorization_version: 1,
                username: "user",
            },
            &settings,
        )
        .unwrap();
        let identity = Arc::new(identity(restricted, permitted));
        let state = AuthState {
            token_settings: settings,
            allow_multiple_tenants: false,
            access_revocations: identity.clone(),
            principal_resolver: identity.clone(),
            refresh_sessions: identity,
        };
        let protected = Router::new()
            .route("/posts", any(reached))
            .layer(from_fn(require_permission("system:post:list")));
        let app = if nested {
            let profile = Router::new()
                .route("/password", any(reached))
                .route("/", any(reached))
                .layer(from_fn_with_state(state.clone(), auth_middleware));
            let auth = Router::new()
                .route("/context", any(reached))
                .route("/context/extra", any(reached))
                .route("/logout", any(reached))
                .route("/ws-ticket", any(reached))
                .layer(from_fn_with_state(state.clone(), auth_middleware))
                .nest("/profile", profile);
            Router::new().nest(
                "/api/v1",
                Router::new()
                    .nest("/auth", auth)
                    .nest(
                        "/system",
                        protected
                            .route("/users", any(reached))
                            .layer(from_fn_with_state(state.clone(), auth_middleware)),
                    )
                    .nest(
                        "/message",
                        Router::new()
                            .route("/ws-ticket", any(reached))
                            .layer(from_fn_with_state(state, auth_middleware)),
                    ),
            )
        } else {
            Router::new()
                .route("/api/v1/auth/context", any(reached))
                .layer(from_fn_with_state(state, auth_middleware))
                .layer(from_fn(without_original_uri))
        };
        (app, token)
    }

    async fn without_original_uri(mut request: axum::extract::Request, next: Next) -> Response {
        request.extensions_mut().remove::<OriginalUri>();
        next.run(request).await
    }

    #[test]
    fn nested_restricted_identity_reaches_only_required_session_handlers() {
        runtime().block_on(async {
            let (app, token) = router(true, true, true);
            for (method, path, allowed) in [
                (Method::GET, "/api/v1/auth/context", true),
                (Method::PUT, "/api/v1/auth/profile/password", true),
                (Method::POST, "/api/v1/auth/logout", true),
                (Method::GET, "/api/v1/system/users", false),
                (Method::GET, "/api/v1/system/posts", false),
                (Method::GET, "/api/v1/auth/profile", false),
                (Method::PUT, "/api/v1/auth/profile", false),
                (Method::POST, "/api/v1/message/ws-ticket", false),
                (Method::POST, "/api/v1/auth/ws-ticket", false),
                (Method::GET, "/api/v1/auth/context/extra", false),
                (Method::POST, "/api/v1/auth/profile/password", false),
                (Method::GET, "/api/v1/auth/logout", false),
            ] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(path)
                            .header("Authorization", format!("Bearer {token}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    if allowed {
                        StatusCode::OK
                    } else {
                        StatusCode::FORBIDDEN
                    },
                    "{path}"
                );
                let body = to_bytes(response.into_body(), 4096).await.unwrap();
                if allowed {
                    assert_eq!(body.as_ref(), "已进入处理器".as_bytes());
                } else {
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(body["error_key"], "permission_denied");
                }
            }
        });
    }

    #[test]
    fn complete_identity_still_requires_business_permission_and_flat_uri_is_supported() {
        runtime().block_on(async {
            for permitted in [false, true] {
                let (app, token) = router(false, permitted, true);
                let response = app
                    .oneshot(
                        Request::builder()
                            .uri("/api/v1/system/posts")
                            .header("Authorization", format!("Bearer {token}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    if permitted {
                        StatusCode::OK
                    } else {
                        StatusCode::FORBIDDEN
                    }
                );
            }
            let (app, token) = router(true, false, false);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/auth/context")
                        .header("Authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        });
    }
}
