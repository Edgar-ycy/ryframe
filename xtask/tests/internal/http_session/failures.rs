use std::time::Duration;

use serde_json::json;

use super::{Hook, Reply, Server, Session, assert_redacted, fixture, run, session};

#[test]
fn before_failure_sends_nothing_and_does_not_complete() {
    run(async {
        let server = Server::start(vec![]);
        let mut session = session(&server);
        let mut hook = Hook {
            fail_before: true,
            ..Default::default()
        };
        let error = session
            .request(&json!({"operation": "get_auth_context"}), &mut hook)
            .await
            .unwrap_err();
        assert_eq!(error.category, "hook_before");
        assert_redacted(&error);
        assert_eq!((hook.before, hook.complete), (1, 0));
        assert!(server.seen.lock().unwrap().is_empty());
    });
}

#[test]
fn completed_requests_call_complete_once_and_preserve_http_failure_if_both_fail() {
    run(async {
        for (status, complete_failed, category) in [
            (200, false, None),
            (200, true, Some("hook_complete")),
            (500, false, Some("http_status")),
            (500, true, Some("http_status")),
        ] {
            let mut reply = Reply::json(json!({"code": 200, "message": "error-body-secret"}));
            reply.status = status;
            let server = Server::start(vec![reply]);
            let mut session = session(&server);
            let mut hook = Hook {
                fail_complete: complete_failed,
                ..Default::default()
            };
            let result = session
                .request(&json!({"operation": "get_auth_context"}), &mut hook)
                .await;
            match category {
                None => {
                    result.unwrap();
                }
                Some(category) => {
                    let error = result.unwrap_err();
                    assert_eq!(error.category, category);
                    assert_redacted(&error);
                }
            }
            assert_eq!((hook.before, hook.complete), (1, 1));
            assert_eq!(server.seen.lock().unwrap().len(), 1);
        }
    });
}

#[test]
fn redirects_are_reported_without_contacting_the_location() {
    run(async {
        let destination = Server::start(vec![Reply::json(json!({"code": 200}))]);
        let mut redirect = Reply::json(json!({"error": "error-body-secret"}));
        redirect.status = 302;
        redirect.headers.push((
            "Location".into(),
            format!("{}/private?token=access-secret", destination.url),
        ));
        let server = Server::start(vec![redirect]);
        let mut session = session(&server);
        let mut hook = Hook::default();
        let error = session
            .request(&json!({"operation": "get_auth_context"}), &mut hook)
            .await
            .unwrap_err();
        assert_eq!(error.category, "http_status");
        assert_eq!(error.status, Some(302));
        assert_redacted(&error);
        assert_eq!((hook.before, hook.complete), (1, 1));
        assert_eq!(server.seen.lock().unwrap().len(), 1);
        assert!(destination.seen.lock().unwrap().is_empty());
    });
}

#[test]
fn headers_and_entire_body_share_the_request_timeout() {
    run(async {
        for delay_headers in [true, false] {
            let mut reply = Reply::json(json!({"code": 200}));
            if delay_headers {
                reply.header_delay = Duration::from_millis(250);
            } else {
                reply.body_delay = Duration::from_millis(250);
            }
            let server = Server::start(vec![reply]);
            let mut session = Session::new(
                &fixture::config(&server.url, 50),
                fixture::catalog(),
                &fixture::identity(),
            )
            .unwrap();
            let mut hook = Hook::default();
            let error = session
                .request(&json!({"operation": "get_auth_context"}), &mut hook)
                .await
                .unwrap_err();
            assert_eq!(error.category, "request_timeout");
            assert_redacted(&error);
            assert_eq!((hook.before, hook.complete), (1, 1));
        }
    });
}

#[test]
fn malformed_non_json_and_business_error_bodies_are_not_exposed() {
    run(async {
        let mut invalid_json = Reply::json(json!(null));
        invalid_json.body = b"error-body-secret".to_vec();
        let mut non_json = Reply::json(json!({"password": fixture::PASSWORD}));
        non_json.headers = vec![("Content-Type".into(), "text/plain".into())];
        for reply in [
            invalid_json,
            non_json,
            Reply::json(json!({"code": 403, "message": "error-body-secret"})),
        ] {
            let server = Server::start(vec![reply]);
            let mut session = session(&server);
            let mut hook = Hook::default();
            let error = session
                .request(&json!({"operation": "get_auth_context"}), &mut hook)
                .await
                .unwrap_err();
            assert!(matches!(
                error.category,
                "invalid_response" | "business_response"
            ));
            assert_redacted(&error);
            assert_eq!((hook.before, hook.complete), (1, 1));
        }
    });
}

#[test]
fn declared_and_streamed_json_limits_allow_exactly_sixteen_mib() {
    run(async {
        let limit = 16 * 1024 * 1024;
        for (size, chunked, succeeds) in [
            (limit, false, true),
            (limit + 1, false, false),
            (limit + 1, true, false),
        ] {
            let mut reply = Reply::json(json!(null));
            reply.body.resize(size, b' ');
            reply.chunked = chunked;
            let server = Server::start(vec![reply]);
            let mut session = Session::new(
                &fixture::config(&server.url, 5000),
                fixture::catalog(),
                &fixture::identity(),
            )
            .unwrap();
            let mut hook = Hook::default();
            let result = session
                .request(&json!({"operation": "get_auth_context"}), &mut hook)
                .await;
            if succeeds {
                assert_eq!(result.unwrap(), serde_json::Value::Null);
            } else {
                assert_eq!(result.unwrap_err().category, "invalid_response");
            }
            assert_eq!((hook.before, hook.complete), (1, 1));
        }
    });
}

#[test]
fn no_content_returns_null_and_transport_failure_still_completes() {
    run(async {
        let mut reply = Reply::json(json!(null));
        reply.status = 204;
        reply.body.clear();
        let server = Server::start(vec![reply]);
        let mut session = session(&server);
        let mut hook = Hook::default();
        assert_eq!(
            session
                .request(&json!({"operation": "get_auth_context"}), &mut hook)
                .await
                .unwrap(),
            json!(null)
        );
        assert_eq!((hook.before, hook.complete), (1, 1));
        let mut disconnect = Reply::json(json!(null));
        disconnect.disconnect = true;
        let disconnected_server = Server::start(vec![disconnect]);
        let mut disconnected = Session::new(
            &fixture::config(&disconnected_server.url, 200),
            fixture::catalog(),
            &fixture::identity(),
        )
        .unwrap();
        let error = disconnected
            .request(&json!({"operation": "get_auth_context"}), &mut hook)
            .await
            .unwrap_err();
        assert!(matches!(
            error.category,
            "request_failed" | "request_timeout"
        ));
        assert_redacted(&error);
        assert_eq!((hook.before, hook.complete), (2, 2));
        assert_eq!(disconnected_server.seen.lock().unwrap().len(), 1);
    });
}

#[test]
fn unauthorized_json_write_is_never_refreshed_or_replayed() {
    run(async {
        let mut denied = Reply::json(json!({"message": "error-body-secret"}));
        denied.status = 401;
        let server = Server::start(vec![
            Reply::json(json!({"code": 200})),
            Reply::json(json!({"code": 200, "data": {"access_token": "access-secret"}})),
            denied,
        ]);
        let mut session = session(&server);
        let mut hook = Hook::default();
        session.login(&mut hook).await.unwrap();
        let error = session
            .request(
                &json!({"operation": "put_monitor_schedules_by_id_status",
            "path": {"id": "9223372036854775807"}, "body": {"enabled": false, "version": "1"}}),
                &mut hook,
            )
            .await
            .unwrap_err();
        assert_eq!(error.category, "http_status");
        assert_eq!(error.status, Some(401));
        assert_redacted(&error);
        assert_eq!((hook.before, hook.complete), (3, 3));
        let calls = server.seen.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[2].method, "PUT");
        assert_eq!(
            calls[2].target,
            "/api/v1/monitor/schedules/9223372036854775807/status"
        );
    });
}
