use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::{
    Hook, NoopHook, Reply, Server, Session, SessionError, assert_redacted, fixture, run, session,
};

#[test]
fn selected_operation_paths_match_the_current_openapi_document() {
    let document: Value =
        serde_json::from_str(include_str!("../../../../openapi/openapi.json")).unwrap();
    let operations = fixture::catalog();
    for operation in [
        "get_auth_context",
        "get_monitor_schedules_by_id",
        "put_monitor_schedules_by_id_status",
        "delete_system_online_by_sid",
    ] {
        let entry = &operations[operation];
        let method = entry["method"].as_str().unwrap().to_ascii_lowercase();
        let route = entry["path"].as_str().unwrap();
        assert_eq!(
            document["paths"][route][&method]["operationId"],
            json!(operation)
        );
    }
}

#[test]
fn path_encoding_query_scalars_and_snowflake_strings_remain_exact() {
    run(async {
        let server = Server::start(vec![
            Reply::json(json!({"code": 200})),
            Reply::json(json!({"code": 200})),
        ]);
        let mut session = session(&server);
        let mut hook = Hook::default();
        let id = "a/b ?#雪%!'()*~+";
        session
            .request(
                &json!({"operation": "get_monitor_schedules_by_id", "path": {"id": id},
            "query": {"zero": 0, "enabled": false, "id": "9223372036854775807", "absent": null}}),
                &mut hook,
            )
            .await
            .unwrap();
        session.request(&json!({"operation": "put_monitor_schedules_by_id_status",
            "path": {"id": "9223372036854775807"}, "body": {"enabled": false, "version": "9223372036854775807"}}),
            &mut hook).await.unwrap();
        let calls = server.seen.lock().unwrap();
        assert!(
            calls[0]
                .target
                .starts_with("/api/v1/monitor/schedules/a%2Fb%20%3F%23%E9%9B%AA%25!'()*~%2B?")
        );
        let url = url::Url::parse(&format!("http://localhost{}", calls[0].target)).unwrap();
        let query = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(query["zero"], "0");
        assert_eq!(query["enabled"], "false");
        assert_eq!(query["id"], "9223372036854775807");
        assert!(!query.contains_key("absent"));
        assert_eq!(calls[1].method, "PUT");
        assert_eq!(
            calls[1].target,
            "/api/v1/monitor/schedules/9223372036854775807/status"
        );
        let body: Value = serde_json::from_slice(&calls[1].body).unwrap();
        assert_eq!(body["enabled"], json!(false));
        assert_eq!(body["version"], json!("9223372036854775807"));
        assert_eq!(
            hook.metadata[0],
            (
                "get_monitor_schedules_by_id".into(),
                "GET".into(),
                "/api/v1/monitor/schedules/{id}".into()
            )
        );
        assert_eq!((hook.before, hook.complete), (2, 2));
    });
}

#[test]
fn benchmark_forwarding_and_explicit_department_idempotency_are_bounded() {
    run(async {
        let server = Server::start(vec![
            Reply::json(json!({"code": 200})),
            Reply::json(json!({"code": 200})),
        ]);
        let mut identity = fixture::identity();
        identity["client_address"] = json!("198.19.255.0");
        let mut session = Session::new(
            &fixture::config(&server.url, 2000),
            fixture::catalog(),
            &identity,
        )
        .unwrap();
        let key = "12345678-1234-4123-8123-123456789abc";
        let mut noop = NoopHook;
        session
            .request(
                &json!({"operation": "post_system_depts", "idempotency_key": key,
            "body": {"name": "测试部门", "enabled": false}}),
                &mut noop,
            )
            .await
            .unwrap();
        session
            .request(&json!({"operation": "get_auth_context"}), &mut noop)
            .await
            .unwrap();
        let calls = server.seen.lock().unwrap();
        assert_eq!(calls[0].headers["idempotency-key"], key);
        assert_eq!(calls[0].headers["x-forwarded-for"], "198.19.255.0");
        assert_ne!(calls[1].headers["idempotency-key"], key);
        assert_eq!(
            calls
                .iter()
                .map(|call| &call.headers["idempotency-key"])
                .collect::<BTreeSet<_>>()
                .len(),
            2
        );
    });
}

#[test]
fn unsupported_steps_and_invalid_parameters_fail_without_hooks_or_http() {
    run(async {
        let server = Server::start(vec![]);
        let mut catalog = fixture::catalog();
        catalog.insert(
            "delete_example".into(),
            json!({"method": "DELETE", "path": "/api/v1/example"}),
        );
        let mut session = Session::new(
            &fixture::config(&server.url, 1000),
            catalog,
            &fixture::identity(),
        )
        .unwrap();
        let mut hook = Hook::default();
        for step in [
            json!({"operation": "get_auth_context", "multipart": []}),
            json!({"operation": "get_auth_context", "binary": false}),
            json!({"operation": "post_auth_refresh", "poll": {}}),
            json!({"operation": "get_auth_context", "workflow": {}}),
            json!({"operation": "get_auth_context", "steps": []}),
            json!({"operation": "delete_example"}),
            json!({"operation": "delete_system_online_by_sid", "path": {"sid":fixture::SESSION_ID}, "idempotency_key":fixture::FORCE_LOGOUT_KEY}),
            json!({"operation": "get_monitor_schedules_by_id"}),
            json!({"operation": "get_monitor_schedules_by_id", "path": {"id": ".."}}),
            json!({"operation": "get_auth_context", "query": {"filter": []}}),
            json!({"operation": "get_auth_context", "body": {}}),
            json!({"operation": "get_auth_context", "idempotency_key": "12345678-1234-4123-8123-123456789abc"}),
            json!({"operation": "post_system_depts", "idempotency_key": "not-a-uuid"}),
            json!({"operation": "https://private.invalid/?token=access-secret"}),
            json!({"operation": "unknown_operation"}),
        ] {
            let error = session.request(&step, &mut hook).await.unwrap_err();
            assert_redacted(&error);
        }
        assert_eq!((hook.before, hook.complete), (0, 0));
        assert!(server.seen.lock().unwrap().is_empty());
    });
}

#[test]
fn force_logout_rejects_unbound_sid_route_key_or_missing_login_before_http() {
    run(async {
        let server = Server::start(vec![]);
        let config = fixture::config(&server.url, 1000);
        let mut hook = Hook::default();
        for (method, path, sid, key, category) in [
            (
                "DELETE",
                "/api/v1/system/online/{sid}",
                "s-../other",
                fixture::FORCE_LOGOUT_KEY,
                "invalid_request",
            ),
            (
                "DELETE",
                "/api/v1/system/online/{sid}",
                fixture::SESSION_ID,
                "invalid",
                "invalid_request",
            ),
            (
                "DELETE",
                "/api/v1/other/{sid}",
                fixture::SESSION_ID,
                fixture::FORCE_LOGOUT_KEY,
                "unsupported_step",
            ),
            (
                "POST",
                "/api/v1/system/online/{sid}",
                fixture::SESSION_ID,
                fixture::FORCE_LOGOUT_KEY,
                "unsupported_step",
            ),
            (
                "DELETE",
                "/api/v1/system/online/{sid}",
                fixture::SESSION_ID,
                fixture::FORCE_LOGOUT_KEY,
                "missing_credentials",
            ),
        ] {
            let mut catalog = fixture::catalog();
            catalog.insert(
                "delete_system_online_by_sid".into(),
                json!({"method":method,"path":path}),
            );
            let mut session = Session::new(&config, catalog, &fixture::identity()).unwrap();
            let outcome = session
                .force_logout_online_session(sid, key, &mut hook)
                .await;
            assert!(!outcome.request_started);
            assert!(outcome.status.is_none() && outcome.request_id.is_none());
            let error = outcome.result.unwrap_err();
            assert_eq!(error.category, category);
            assert_redacted(&error);
        }
        assert_eq!((hook.before, hook.complete), (0, 0));
        assert!(server.seen.lock().unwrap().is_empty());
    });
}

#[test]
fn invalid_configuration_catalog_and_identity_errors_are_redacted() {
    for endpoint in [
        "https://user:access-secret@private.invalid",
        "ftp://private.invalid",
        "http://127.0.0.1/?token=access-secret",
    ] {
        let error = Session::new(
            &fixture::config(endpoint, 1000),
            fixture::catalog(),
            &fixture::identity(),
        )
        .err()
        .unwrap();
        assert_redacted(&error);
    }
    for address in [
        "127.0.0.1",
        "198.20.0.1",
        "198.18.256.0",
        "198.18.1.1\r\nAuthorization: access-secret",
    ] {
        let mut identity = fixture::identity();
        identity["client_address"] = json!(address);
        assert!(
            Session::new(
                &fixture::config("http://127.0.0.1:1", 1000),
                fixture::catalog(),
                &identity,
            )
            .is_err()
        );
    }
    for endpoint in [
        "http://localhost:1",
        "https://private.invalid",
        "http://127.0.0.2:1",
    ] {
        let mut identity = fixture::identity();
        identity["client_address"] = json!("198.18.1.1");
        assert!(
            Session::new(
                &fixture::config(endpoint, 1000),
                fixture::catalog(),
                &identity,
            )
            .is_err()
        );
    }
    for route in [
        "//private.invalid",
        "/api/../outside",
        "/api/%2e%2e/outside",
        "/api/{id",
        "/api/{bad/key}",
        "/api/route?token=access-secret",
        "/api/\\outside",
    ] {
        let mut catalog = fixture::catalog();
        catalog.insert("bad_route".into(), json!({"method": "GET", "path": route}));
        let error = Session::new(
            &fixture::config("http://127.0.0.1:1", 1000),
            catalog,
            &fixture::identity(),
        )
        .err()
        .unwrap();
        assert_redacted(&error);
    }
    let error = SessionError {
        category: "https://private.invalid/?token=access-secret",
        operation: Some("https://private.invalid/?password=error-body-secret".into()),
        status: Some(999),
    };
    assert_redacted(&error);
    assert_eq!(error.to_string(), "request_failed");
}
