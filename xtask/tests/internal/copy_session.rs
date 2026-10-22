use std::io::Cursor;

use serde_json::{Value, json};

use crate::recovery::clone::native::session::{model, run};

fn identity() -> Value {
    json!({"subject_id":"100", "tenant_id":"system", "username":"operator",
        "password":"direct-password-sentinel", "client_address":"198.19.20.12"})
}

#[test]
fn authorization_requires_exact_identity_and_actual_schedule_permissions() {
    let identity = identity();
    let mut response = json!({"data": {"user":{"id":"100", "tenant_id":"system", "username":"operator"},
        "is_super_admin":false, "permissions":["monitor:schedule:list","monitor:schedule:edit"]}});
    assert!(model::authorization(&response, &identity).is_ok());
    response["data"]["permissions"] = json!(["monitor:schedule:list"]);
    assert!(model::authorization(&response, &identity).is_err());
    response["data"]["permissions"] = json!(["*"]);
    assert!(model::authorization(&response, &identity).is_ok());
    response["data"]["user"]["id"] = json!("101");
    assert!(model::authorization(&response, &identity).is_err());
}

#[test]
fn schedule_actions_are_fixed_and_preserve_string_ids_and_version_zero() {
    let value = model::schedule(&json!({"id":2,"operation":"disable",
        "schedule_id":"18446744073709551615","version":0}))
    .unwrap();
    assert_eq!(value["operation"], "put_monitor_schedules_by_id_status");
    assert_eq!(value["path"]["id"], "18446744073709551615");
    assert_eq!(value["body"], json!({"enabled":false,"version":0}));
    for invalid in [
        json!({"id":2,"operation":"delete","schedule_id":"10"}),
        json!({"id":2,"operation":"get","schedule_id":10}),
        json!({"id":2,"operation":"get","schedule_id":"10","url":"http://example.invalid"}),
        json!({"id":2,"operation":"disable","schedule_id":"10","version":-1}),
    ] {
        assert!(model::schedule(&invalid).is_err());
    }
}

#[test]
fn registration_rejects_other_attempt_or_artifact_directory_before_network() {
    let root = tempfile::tempdir().unwrap();
    let run = root.path().join(".local-tests/copy-session");
    std::fs::create_dir_all(&run).unwrap();
    let registration = model::Registration::parse(
        root.path(),
        &[
            "--run-dir".into(),
            run.to_str().unwrap().into(),
            "--attempt".into(),
            "1".into(),
        ],
    )
    .unwrap();
    let artifacts = run.join("post-copy").join("attempt-0001").join("pacing");
    let mut request = json!({"id":1,"operation":"initialize",
        "backend":root.path(),"frontend":root.path(),"run_dir":run,"attempt":1,
        "artifacts":artifacts,"identity":identity(),
        "config":{"contract":{"timeout_ms":1000,"pacing":{}},
            "bindings":{"scope_id":"sample", "api_url":"http://127.0.0.1:1",
                "frontend_url":"http://127.0.0.1:2", "pacing":{}}}});
    std::fs::create_dir_all(run.join("post-copy/attempt-0001")).unwrap();
    registration.initialize(&request).unwrap();
    request["attempt"] = json!(2);
    assert!(registration.initialize(&request).is_err());
    request["attempt"] = json!(1);
    request["artifacts"] = json!(run.join("other"));
    assert!(registration.initialize(&request).is_err());
}

#[test]
fn malformed_or_unauthorized_protocol_never_creates_pacing_or_reads_password() {
    let root = tempfile::tempdir().unwrap();
    let registration = model::Registration {
        root: root.path().into(),
        run: root.path().into(),
        attempt: 1,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for input in [
        b"{\"id\":1,\"id\":2}\n".as_slice(),
        b"{\"id\":2,\"operation\":\"initialize\"}\n",
        b"{\"id\":1,\"operation\":\"get\",\"schedule_id\":\"1\"}\n",
        b"{\"id\":1,\"operation\":\"initialize\",\"password\":\"secret-sentinel\"}\n",
    ] {
        let mut output = Vec::new();
        assert!(
            runtime
                .block_on(run(&registration, &mut Cursor::new(input), &mut output))
                .is_err()
        );
        assert!(
            !String::from_utf8(output)
                .unwrap()
                .contains("secret-sentinel")
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn direct_password_is_required_and_preserved_without_entering_authorization_output() {
    for password in ["direct-password-sentinel", " 中文密码与空格 ", " "] {
        let mut selected = identity();
        selected["password"] = json!(password);
        let before = selected.clone();
        assert!(model::identity(&selected).is_ok());
        assert_eq!(selected, before);
        let response = json!({"data": {
            "user":{"id":"100", "tenant_id":"system", "username":"operator"},
            "is_super_admin":false, "permissions":["*"]
        }});
        let authorization = model::authorization(&response, &selected).unwrap();
        assert!(authorization.get("password").is_none());
        assert!(authorization.get("password_env").is_none());
        assert_eq!(authorization.as_object().unwrap().len(), 5);
    }
    for password in [
        json!(""),
        Value::Null,
        json!(1),
        json!(false),
        json!([]),
        json!({}),
    ] {
        let mut selected = identity();
        selected["password"] = password;
        assert_eq!(model::identity(&selected), Err("MissingSessionCredentials"));
    }
}

#[test]
fn old_password_reference_and_unknown_identity_fields_are_rejected_without_values() {
    for keep_password in [false, true] {
        let mut selected = identity();
        if !keep_password {
            selected.as_object_mut().unwrap().remove("password");
        }
        selected["password_env"] = json!("PRIVATE_PASSWORD_REFERENCE_SENTINEL");
        let error = model::identity(&selected).unwrap_err();
        assert_eq!(error, "InvalidSessionRequest");
        assert!(!error.contains("PRIVATE_PASSWORD_REFERENCE_SENTINEL"));
        assert!(!error.contains("direct-password-sentinel"));
    }
    let mut selected = identity();
    selected.as_object_mut().unwrap().remove("password");
    assert_eq!(model::identity(&selected), Err("InvalidSessionRequest"));
    let mut selected = identity();
    selected["extra"] = json!("private-identity-sentinel");
    assert_eq!(model::identity(&selected), Err("InvalidSessionRequest"));
}

#[test]
fn bindings_allow_optional_literal_configuration_directory_and_reject_old_or_unknown_fields() {
    let base = json!({"scope_id":"sample", "api_url":"http://127.0.0.1:1",
        "frontend_url":"http://127.0.0.1:2", "pacing":{}});
    assert!(model::bindings(&base).is_ok());
    for directory in ["config", "角色 配置/中文 目录", "D:/角色 配置/目录"] {
        let mut bindings = base.clone();
        bindings["app_config_dir"] = json!(directory);
        let before = bindings.clone();
        assert!(model::bindings(&bindings).is_ok());
        assert_eq!(bindings, before);
    }
    for directory in [
        json!(""),
        json!("bad\npath"),
        json!("bad\rpath"),
        json!("bad\tpath"),
        json!("bad\0path"),
        json!("bad\u{7f}path"),
        Value::Null,
        json!(1),
        json!(true),
        json!([]),
        json!({}),
    ] {
        let mut bindings = base.clone();
        bindings["app_config_dir"] = directory;
        assert_eq!(model::bindings(&bindings), Err("InvalidAppConfigDirectory"));
    }
    for field in [
        "app_env",
        "APP_ENV",
        "APP_CONFIG_DIR",
        "environment",
        "extra",
    ] {
        let mut bindings = base.clone();
        bindings[field] = json!("private-config-sentinel");
        assert_eq!(model::bindings(&bindings), Err("InvalidSessionRequest"));
    }
    for field in ["scope_id", "api_url", "frontend_url", "pacing"] {
        let mut bindings = base.clone();
        bindings.as_object_mut().unwrap().remove(field);
        assert_eq!(model::bindings(&bindings), Err("InvalidSessionRequest"));
    }
}
