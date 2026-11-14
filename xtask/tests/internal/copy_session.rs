use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::recovery::clone::native::{
    digest, json as document,
    lifecycle::{identity as process_identity, lock::RunLock, state},
    resources::common,
    session::{model, run},
    target_verify::input::request_configuration::canonical_path,
};

fn identity() -> Value {
    json!({"subject_id":"100", "tenant_id":"system", "username":"operator",
        "client_address":"198.19.20.12"})
}

#[test]
fn authorization_requires_exact_identity_and_actual_schedule_permissions() {
    let identity = identity();
    let mut response = json!({"data": {"user":{"id":"100", "tenant_id":"system", "username":"operator"},
        "password_change_required":false, "is_super_admin":false, "permissions":["monitor:schedule:list","monitor:schedule:edit"]}});
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
fn public_identity_is_valid_and_authorization_never_contains_credentials() {
    let selected = identity();
    assert!(model::identity(&selected).is_ok());
    let response = json!({"data": {
        "user":{"id":"100", "tenant_id":"system", "username":"operator"},
        "password_change_required":false, "is_super_admin":false, "permissions":["*"]
    }});
    let authorization = model::authorization(&response, &selected).unwrap();
    assert!(authorization.get("password").is_none());
    assert!(authorization.get("password_env").is_none());
    assert_eq!(authorization.as_object().unwrap().len(), 5);
}

#[test]
fn passwords_references_and_unknown_identity_fields_are_rejected_without_values() {
    for field in ["password", "password_env"] {
        let mut selected = identity();
        selected[field] = json!("PRIVATE_PASSWORD_REFERENCE_SENTINEL");
        let error = model::identity(&selected).unwrap_err();
        assert_eq!(error, "InvalidSessionRequest");
        assert!(!error.contains("PRIVATE_PASSWORD_REFERENCE_SENTINEL"));
    }
    let mut selected = identity();
    selected.as_object_mut().unwrap().remove("username");
    assert_eq!(model::identity(&selected), Err("InvalidSessionRequest"));
    let mut selected = identity();
    selected["extra"] = json!("private-identity-sentinel");
    assert_eq!(model::identity(&selected), Err("InvalidSessionRequest"));
}

#[test]
fn bindings_allow_optional_literal_configuration_directory_and_reject_old_or_unknown_fields() {
    let root = crate::workspace::root_dir();
    let base = json!({"scope_id":"sample", "api_url":"http://127.0.0.1:1",
        "frontend_url":"http://127.0.0.1:2",
        "pacing":{"login_budget_state":root.join(".local-tests/copy-session/login-budget.json")}});
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

#[test]
fn current_credentials_require_authoritative_password_change_completion() {
    let selected = identity();
    let base = json!({"data":{"user":{"id":"100","tenant_id":"system","username":"operator"},
        "password_change_required":false,"is_super_admin":true,"permissions":["*"]}});
    assert!(model::authorization(&base, &selected).is_ok());
    for value in [Value::Null, json!(true), json!("false"), json!(0)] {
        let mut response = base.clone();
        response["data"]["password_change_required"] = value;
        assert_eq!(
            model::authorization(&response, &selected),
            Err("SessionPasswordChangeRequired")
        );
    }
    let mut missing = base;
    missing["data"]
        .as_object_mut()
        .unwrap()
        .remove("password_change_required");
    assert_eq!(
        model::authorization(&missing, &selected),
        Err("SessionPasswordChangeRequired")
    );
}

// 仅构造历史解析器消费的发布事实；不代表真实 amendment 生命周期验收。
struct ActiveSessionFixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    run: PathBuf,
    base: Value,
    post: Value,
    active: Value,
    lock: Option<RunLock>,
}

impl ActiveSessionFixture {
    fn new(amended: bool, failed_latest: bool) -> Self {
        let parent = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let temporary = tempfile::Builder::new()
            .prefix("active-session-")
            .tempdir_in(parent)
            .unwrap();
        let root = canonical_path(temporary.path()).unwrap();
        let run = root.join(".local-tests").join("run");
        fs::create_dir_all(run.join("post-copy").join("amendments")).unwrap();
        document::write_new(&run.join("manifest.json"), &json!({})).unwrap();
        state::initialize(&run).unwrap();
        let credentials = run.join("administrator.toml");
        fs::write(&credentials, "[administrator]\ntenant_id = \"system\"\nusername = \"operator\"\npassword = \"PRIVATE_SESSION_PASSWORD_SENTINEL\"\n").unwrap();
        let environment = run.join("environment.json");
        document::write_new(&environment, &json!({"environment":{"APP_ENV":"test"}})).unwrap();
        let fact = common::binding(&environment).unwrap();
        let pacing = json!({"contract":{},"bindings":{"login_budget_state":run.join("post-copy/login-budget.json")},
            "login_budget_module_sha256":"a".repeat(64)});
        let post = json!({"format_version":1,"kind":"devex-clone-post-copy",
            "run_manifest":state::manifest_binding(&run).unwrap(),"copy_stage_receipt":fact,
            "copy_result":fact,"ledger_head":fact,"api_environment":fact,"backend_build":fact,
            "node":fact,"source_admin":identity(),"frontend_root":root,"pacing":pacing,
            "reference_plan":fact,"dataset":fact,"administrator_credentials":common::binding(&credentials).unwrap()});
        document::write_new(&run.join("post-copy.json"), &post).unwrap();
        let base = common::binding(&run.join("post-copy.json")).unwrap();
        let mut fixture = Self {
            _temporary: temporary,
            root,
            run,
            base: base.clone(),
            post,
            active: base,
            lock: None,
        };
        fixture.publish(
            "post-copy",
            "register",
            &json!({"status":"post_copy_registered","registration":fixture.base}),
        );
        if amended {
            fixture.amend();
        }
        if failed_latest {
            let (number, _) = state::begin(&fixture.run, "post-copy", "amend", &json!({})).unwrap();
            state::finish(&fixture.run, number, None, Some("PostCopyRejected")).unwrap();
        }
        fixture
    }

    fn publish(&self, stage: &str, mode: &str, result: &Value) -> u64 {
        let (number, _) = state::begin(&self.run, stage, mode, &json!({})).unwrap();
        state::finish(&self.run, number, Some(result), None).unwrap();
        number
    }

    fn amend(&mut self) {
        let runtime = self.run.join("post-copy").join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        let mut invalidated = json!({});
        for (field, name) in [
            ("handoff", "handoff.json"),
            ("runtime", "runtime.json"),
            ("binaries", "binaries.json"),
            ("api_process", "api.json"),
            ("producer_history", "producer-history.json"),
        ] {
            let path = runtime.join(name);
            document::write_new(&path, &json!({"unit_published_fact":name})).unwrap();
            invalidated[field] = common::binding(&path).unwrap();
        }
        let prepared = self.publish(
            "post-copy",
            "prepare",
            &json!({"status":"post_copy_api_prepared"}),
        );
        invalidated["prepare"] = common::binding(&state::result_path(&self.run, prepared)).unwrap();
        let lock = RunLock::claim(&self.run).unwrap();
        let (failed, attempt) =
            state::begin(&self.run, "runtime-target", "start", &json!({})).unwrap();
        state::bind_controller(&self.run, failed, &lock.owner, &attempt).unwrap();
        let failure = self.run.join(format!("failure-{failed:04}.json"));
        document::write_new(&failure, &json!({"error_type":"UnitPublishedFailure"})).unwrap();
        state::finish(&self.run, failed, None, Some("UnitPublishedFailure")).unwrap();
        lock.release().unwrap();
        let stopped = self.publish(
            "runtime-target",
            "stop",
            &json!({"status":"runtime_stopped"}),
        );
        invalidated["failed_start"] = json!({"attempt":failed,
            "controller":common::binding(&state::controller_path(&self.run, failed)).unwrap(),
            "failure":common::binding(&failure).unwrap()});
        invalidated["stopped"] = json!({"attempt":stopped,"result":common::binding(&state::result_path(&self.run, stopped)).unwrap()});
        let environment = self.run.join("amended-environment.json");
        document::write_new(&environment, &json!({"environment":{"APP_ENV":"test"}})).unwrap();
        self.post["api_environment"] = common::binding(&environment).unwrap();
        let path = self
            .run
            .join("post-copy")
            .join("amendments")
            .join("0001.json");
        document::write_new(&path, &json!({"format_version":1,"kind":"devex-clone-post-copy-amendment",
            "sequence":1,"run_manifest":state::manifest_binding(&self.run).unwrap(),"predecessor":self.base,
            "request_sha256":digest::plan_hash(&self.post).unwrap(),"api_environment":self.post["api_environment"],
            "invalidated_runtime":invalidated})).unwrap();
        self.active = common::binding(&path).unwrap();
        self.publish("post-copy", "amend", &json!({"status":"post_copy_amended","registration":self.active,
            "sequence":1,"predecessor":self.base,"runtime_directory":self.run.join("post-copy").join("runtime-0001"),
            "remote_writes":0,"worker_must_remain_stopped":true,"restore_qualified":false}));
    }

    fn initialize_request(&mut self) -> (model::Registration, Value) {
        let source = self.root.join("xtask/src/recovery/clone/native/session.rs");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::copy(
            crate::workspace::root_dir().join("xtask/src/recovery/clone/native/session.rs"),
            &source,
        )
        .unwrap();
        let lock = RunLock::claim(&self.run).unwrap();
        let (number, attempt) =
            state::begin(&self.run, "post-copy", "schedules", &json!({})).unwrap();
        state::bind_controller(&self.run, number, &lock.owner, &attempt).unwrap();
        let output = self
            .run
            .join("post-copy")
            .join(format!("attempt-{number:04}"));
        fs::create_dir_all(&output).unwrap();
        let actual = process_identity::current().unwrap();
        let executable = Path::new(actual["executable"].as_str().unwrap());
        let tool = json!({"path":executable,"sha256":digest::file(executable).unwrap().sha256});
        let arguments = [
            "--run-dir".to_owned(),
            self.run.to_string_lossy().into_owned(),
            "--attempt".to_owned(),
            number.to_string(),
        ];
        let mut command = vec![
            tool["path"].as_str().unwrap().to_owned(),
            "__copy-session".to_owned(),
        ];
        command.extend(arguments.iter().cloned());
        document::write_new(&output.join("session-launch.json"), &json!({"format_version":1,
            "kind":"devex-post-producer-launch","run_manifest":state::manifest_binding(&self.run).unwrap(),
            "registration":self.active,"controller":common::binding(&state::controller_path(&self.run, number)).unwrap(),
            "attempt":number,"producer_kind":"session","command":command,"tool":tool,
            "source":common::binding(&source).unwrap()})).unwrap();
        let owner = uuid::Uuid::new_v4().to_string();
        let launch = common::binding(&output.join("session-launch.json")).unwrap();
        document::write_new(
            &output.join("session-process.json"),
            &json!({"format_version":1,
            "kind":"devex-post-producer","launch":launch,"identity":actual,"producer_owner":owner}),
        )
        .unwrap();
        let request = json!({"id":1,"operation":"initialize","backend":self.root,"frontend":self.root,
            "run_dir":self.run,"attempt":number,"producer_owner":owner,"launch":launch,
            "process":common::binding(&output.join("session-process.json")).unwrap(),"artifacts":output.join("pacing"),
            "identity":identity(),"config":{"contract":{"timeout_ms":1000,"pacing":{}},
                "bindings":{"scope_id":"sample","api_url":"http://127.0.0.1:1","frontend_url":"http://127.0.0.1:2",
                    "pacing":self.post["pacing"]["bindings"]}}});
        self.lock = Some(lock);
        (
            model::Registration::parse(&self.root, &arguments).unwrap(),
            request,
        )
    }

    fn rebind_launch(&self, request: &mut Value, descriptor: &Value) {
        let launch_path = PathBuf::from(request["launch"]["path"].as_str().unwrap());
        let mut launch = document::read(&launch_path).unwrap();
        launch["registration"] = descriptor.clone();
        document::write_replace(&launch_path, &launch).unwrap();
        request["launch"] = common::binding(&launch_path).unwrap();
        let process_path = PathBuf::from(request["process"]["path"].as_str().unwrap());
        let mut process = document::read(&process_path).unwrap();
        process["launch"] = request["launch"].clone();
        document::write_replace(&process_path, &process).unwrap();
        request["process"] = common::binding(&process_path).unwrap();
    }
}

impl Drop for ActiveSessionFixture {
    fn drop(&mut self) {
        if let Some(lock) = self.lock.take() {
            lock.release().unwrap();
        }
    }
}

#[test]
fn initialization_authorizes_only_the_active_whole_registration_and_bound_credentials() {
    let producer_mismatch = Some("原生生产者 launch、进程身份或 owner 不符");
    for (case, expected_error) in [
        ("base", None),
        ("amended", None),
        ("base-after-amend", producer_mismatch),
        ("ungranted", Some("InvalidSessionRequest")),
        ("stale-attempt", Some("SessionRegistrationMismatch")),
        ("artifacts-drift", Some("SessionArtifactsMismatch")),
        ("latest-failed", Some("SessionRegistrationRejected")),
        ("foreign", producer_mismatch),
        ("unpublished", producer_mismatch),
        ("registration-drift", Some("SessionRegistrationRejected")),
        ("credentials-drift", Some("管理员凭据校验失败")),
        ("identity-drift", Some("SessionIdentityMismatch")),
    ] {
        let mut fixture = ActiveSessionFixture::new(case != "base", case == "latest-failed");
        let (registration, mut request) = fixture.initialize_request();
        match case {
            "base-after-amend" => fixture.rebind_launch(&mut request, &fixture.base),
            "ungranted" => {
                request.as_object_mut().unwrap().remove("producer_owner");
            }
            "stale-attempt" => request["attempt"] = json!(registration.attempt - 1),
            "artifacts-drift" => request["artifacts"] = json!(fixture.run.join("other")),
            "foreign" | "unpublished" => {
                let destination = if case == "foreign" {
                    fixture.run.join("foreign.json")
                } else {
                    fixture
                        .run
                        .join("post-copy")
                        .join("amendments")
                        .join("0002.json")
                };
                fs::copy(fixture.active["path"].as_str().unwrap(), &destination).unwrap();
                fixture.rebind_launch(&mut request, &common::binding(&destination).unwrap());
            }
            "registration-drift" => fs::write(
                fixture.active["path"].as_str().unwrap(),
                b"PRIVATE_REGISTRATION_SENTINEL",
            )
            .unwrap(),
            "credentials-drift" => fs::write(
                fixture.post["administrator_credentials"]["path"]
                    .as_str()
                    .unwrap(),
                b"PRIVATE_CREDENTIAL_DRIFT_SENTINEL",
            )
            .unwrap(),
            "identity-drift" => request["identity"]["subject_id"] = json!("101"),
            _ => {}
        }
        let state_before = fs::read(fixture.run.join("state.json")).unwrap();
        let active_before = fs::read(fixture.active["path"].as_str().unwrap()).unwrap();
        match registration.initialize(&request) {
            Ok(initialized) => {
                assert!(expected_error.is_none(), "{case}");
                let mut expected = identity();
                expected["password"] = json!("PRIVATE_SESSION_PASSWORD_SENTINEL");
                assert_eq!(initialized.identity, expected, "{case}");
                assert_eq!(
                    initialized.artifacts,
                    Path::new(request["artifacts"].as_str().unwrap())
                );
            }
            Err(error) => {
                let message = error.to_string();
                assert!(!message.contains("SENTINEL"), "{case}: {message}");
                let reason = expected_error.unwrap_or_else(|| panic!("{case}: {message}"));
                assert!(message.contains(reason), "{case}: {message}");
            }
        }
        assert_eq!(
            fs::read(fixture.run.join("state.json")).unwrap(),
            state_before,
            "{case}"
        );
        assert_eq!(
            fs::read(fixture.active["path"].as_str().unwrap()).unwrap(),
            active_before,
            "{case}"
        );
        assert!(
            !Path::new(request["artifacts"].as_str().unwrap()).exists(),
            "{case}"
        );
        fixture.lock.as_ref().unwrap().verify().unwrap();
    }
}
