use serde_json::json;

use super::{Hook, Reply, Server, assert_redacted, fixture, run, session};

#[test]
fn force_logout_sends_one_exact_authenticated_delete_with_stable_key() {
    run(async {
        let mut accepted = Reply::json(json!({"code": 200}));
        accepted
            .headers
            .push(("X-Request-Id".into(), fixture::SERVER_REQUEST_ID.into()));
        let server = Server::start(vec![
            Reply::json(json!({"code": 200})).cookie("ryframe_csrf=anonymous"),
            Reply::json(json!({"code": 200, "data": {"access_token": "access-secret"}}))
                .cookie("ryframe_refresh_token=refresh-secret"),
            Reply::json(json!({"code": 200})).cookie("ryframe_csrf=bound-force-logout"),
            accepted,
        ]);
        let mut session = session(&server);
        let mut hook = Hook::default();
        session.login(&mut hook).await.unwrap();

        let outcome = session
            .force_logout_online_session(fixture::SESSION_ID, fixture::FORCE_LOGOUT_KEY, &mut hook)
            .await;
        assert!(outcome.result.is_ok());
        assert!(outcome.request_started);
        assert_eq!(outcome.status, Some(200));
        assert_eq!(
            outcome.request_id.as_deref(),
            Some(fixture::SERVER_REQUEST_ID)
        );
        assert_eq!((hook.before, hook.complete), (4, 4));
        assert_eq!(hook.metadata[3].0, "delete_system_online_by_sid");
        let calls = server.seen.lock().unwrap();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[2].target, "/api/v1/auth/csrf");
        let delete = &calls[3];
        assert_eq!(delete.method, "DELETE");
        assert_eq!(
            delete.target,
            format!("/api/v1/system/online/{}", fixture::SESSION_ID)
        );
        assert_eq!(delete.headers["x-tenant-id"], "tenant-a");
        assert_eq!(delete.headers["origin"], "http://localhost:4174");
        assert_eq!(delete.headers["x-csrf-token"], "bound-force-logout");
        assert_eq!(delete.headers["authorization"], "Bearer access-secret");
        assert_eq!(delete.headers["idempotency-key"], fixture::FORCE_LOGOUT_KEY);
        assert!(!delete.headers.contains_key("x-request-id"));
        assert!(delete.body.is_empty());
    });
}

#[test]
fn csrf_login_refresh_context_and_logout_use_fresh_challenges_and_cookies() {
    run(async {
        let server = Server::start(vec![
            Reply::json(json!({"code": 200})).cookie("ryframe_csrf=anonymous; Path=/"),
            Reply::json(json!({"code": 200, "data": {"access_token": "access-secret"}}))
                .cookie("ryframe_refresh_token=refresh-secret; HttpOnly; Path=/api/v1/auth"),
            Reply::json(json!({"code": 200})).cookie("ryframe_csrf=bound-refresh; Path=/"),
            Reply::json(json!({"code": 200, "data": {"access_token": "access-second"}})),
            Reply::json(json!({"code": 200, "data": {"subject_id": "9223372036854775807"}})),
            Reply::json(json!({"code": 200})).cookie("ryframe_csrf=bound-logout; Path=/"),
            Reply::json(json!({"code": 200}))
                .cookie("ryframe_refresh_token=; Max-Age=0; Path=/api/v1/auth"),
        ]);
        let mut session = session(&server);
        let mut hook = Hook::default();
        session.login(&mut hook).await.unwrap();
        assert!(session.token() == Some("access-secret"));
        session
            .request(&json!({"operation": "post_auth_refresh"}), &mut hook)
            .await
            .unwrap();
        assert!(session.token() == Some("access-second"));
        let result = session
            .request(&json!({"operation": "get_auth_context"}), &mut hook)
            .await
            .unwrap();
        assert_eq!(result["data"]["subject_id"], json!("9223372036854775807"));
        session
            .request(&json!({"operation": "post_auth_logout"}), &mut hook)
            .await
            .unwrap();
        assert_eq!((hook.before, hook.complete), (7, 7));
        let calls = server.seen.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .map(|call| call.target.as_str())
                .collect::<Vec<_>>(),
            vec![
                "/api/v1/auth/csrf",
                "/api/v1/auth/login",
                "/api/v1/auth/csrf",
                "/api/v1/auth/refresh",
                "/api/v1/auth/context",
                "/api/v1/auth/csrf",
                "/api/v1/auth/logout",
            ]
        );
        assert!(!calls[0].headers.contains_key("authorization"));
        assert!(!calls[0].headers.contains_key("cookie"));
        assert!(calls[1].headers["x-csrf-token"] == "anonymous");
        assert!(calls[3].headers["x-csrf-token"] == "bound-refresh");
        assert!(calls[3].headers["cookie"].contains("ryframe_refresh_token=refresh-secret"));
        assert!(calls[4].headers["authorization"] == "Bearer access-second");
        assert!(calls[6].headers["x-csrf-token"] == "bound-logout");
        for call in calls.iter() {
            assert_eq!(call.headers["origin"], "http://localhost:4174");
            assert_eq!(call.headers["x-tenant-id"], "tenant-a");
            assert_eq!(
                uuid::Uuid::parse_str(&call.headers["idempotency-key"])
                    .unwrap()
                    .get_version_num(),
                4
            );
        }
        let body: serde_json::Value = serde_json::from_slice(&calls[1].body).unwrap();
        assert!(body["password"].as_str() == Some(fixture::PASSWORD));
        assert_eq!(body["username"], json!("bench"));
    });
}

#[test]
fn failed_login_and_invalid_login_payload_never_install_a_token() {
    run(async {
        for (status, body, category) in [
            (401, json!({"detail": "error-body-secret"}), "http_status"),
            (
                200,
                json!({"code": 403, "message": "error-body-secret"}),
                "business_response",
            ),
            (200, json!({"code": 200, "data": {}}), "invalid_response"),
            (
                200,
                json!({"code": 200, "data": {"access_token": 123}}),
                "invalid_response",
            ),
        ] {
            let mut login = Reply::json(body);
            login.status = status;
            let server = Server::start(vec![Reply::json(json!({"code": 200})), login]);
            let mut session = session(&server);
            let mut hook = Hook::default();
            let error = session.login(&mut hook).await.unwrap_err();
            assert_eq!(error.category, category);
            assert_redacted(&error);
            assert!(session.token().is_none());
            assert_eq!((hook.before, hook.complete), (2, 2));
            assert_eq!(server.seen.lock().unwrap().len(), 2);
        }
    });
}

#[test]
fn failed_refresh_is_not_replayed_and_preserves_previous_token() {
    run(async {
        let mut denied = Reply::json(json!({"error": "error-body-secret"}));
        denied.status = 401;
        let server = Server::start(vec![
            Reply::json(json!({"code": 200})),
            Reply::json(json!({"code": 200, "data": {"access_token": "access-secret"}})),
            Reply::json(json!({"code": 200})).cookie("ryframe_csrf=bound"),
            denied,
        ]);
        let mut session = session(&server);
        let mut hook = Hook::default();
        session.login(&mut hook).await.unwrap();
        let error = session
            .request(&json!({"operation": "post_auth_refresh"}), &mut hook)
            .await
            .unwrap_err();
        assert_eq!(error.category, "http_status");
        assert_eq!(error.status, Some(401));
        assert_redacted(&error);
        assert!(session.token() == Some("access-secret"));
        assert_eq!((hook.before, hook.complete), (4, 4));
        assert_eq!(server.seen.lock().unwrap().len(), 4);
    });
}

#[test]
fn missing_password_fails_before_any_hook_or_http() {
    run(async {
        let server = Server::start(vec![]);
        for password in [None, Some("")] {
            let mut identity = fixture::identity();
            match password {
                None => {
                    identity.as_object_mut().unwrap().remove("password");
                }
                Some(value) => {
                    identity["password"] = json!(value);
                }
            }
            let mut session = super::Session::new(
                &fixture::config(&server.url, 1000),
                fixture::catalog(),
                &identity,
            )
            .unwrap();
            let mut hook = Hook::default();
            let error = session.login(&mut hook).await.unwrap_err();
            assert_eq!(error.category, "missing_credentials");
            assert_redacted(&error);
            assert_eq!((hook.before, hook.complete), (0, 0));
        }
        assert!(server.seen.lock().unwrap().is_empty());
    });
}
