use super::*;
use crate::recovery::dataset_prepare::model::Inputs;
const INITIAL: &str = "Initial!Only123";
const CURRENT: &str = "Current!Changed123";

struct InputsFixture {
    _temporary: tempfile::TempDir,
    inputs: Inputs,
    directory: std::path::PathBuf,
    credentials: std::path::PathBuf,
    request: Value,
}
impl InputsFixture {
    fn new(server: &Server) -> Self {
        let temporary = super::super::directory();
        let directory = temporary.path().join("dataset");
        std::fs::create_dir(&directory).unwrap();
        let mut plan = super::super::plan();
        plan["work_dir"] = json!(temporary.path());
        plan["source"]["api_url"] = json!(server.url);
        plan["dataset"]["request_interval_ms"] = json!(1000);
        plan["dataset"]["configuration"] = super::super::configuration(temporary.path());
        plan["dataset"]["administrator_credentials"] = super::super::credentials(temporary.path());
        let credentials = temporary.path().join("administrator.toml");
        let plan_path = temporary.path().join("plan.json");
        let preflight = temporary.path().join("preflight.json");
        std::fs::write(&plan_path, serde_json::to_vec(&plan).unwrap()).unwrap();
        std::fs::write(&preflight, serde_json::to_vec(&json!({"format_version":1,
            "kind":"restore-reference-dataset-preflight","plan_sha256":super::super::model::hash(&plan).unwrap(),
            "side":"source","scope_id":"source-case","actions":{"business":"empty_source_verified"}})).unwrap()).unwrap();
        let root = crate::workspace::root_dir();
        let request = json!({"backend_dir":root,"format_version":1,"kind":crate::recovery::dataset_prepare::KIND,
            "plan":plan_path,"preflight":preflight,"side":"source","verify_existing":null,"write":true});
        let inputs = Inputs::read(&request, &root).unwrap();
        Self {
            _temporary: temporary,
            inputs,
            directory,
            credentials,
            request,
        }
    }
    fn client(&self, index: usize) -> Client {
        let mut client =
            Client::new(&self.inputs.plan, "source", index, &catalog(), INITIAL).unwrap();
        client.pacer = Pacer::new(0);
        client
    }
    fn assert_private(&self) {
        for entry in std::fs::read_dir(&self.directory).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            for secret in [
                INITIAL,
                CURRENT,
                "initial-token",
                "current-token",
                "old_password",
                "new_password",
            ] {
                assert!(!text.contains(secret), "证据泄漏 {secret}");
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Failure {
    None,
    Disconnected,
    Confirmation,
    Subject,
    Permission,
    Wildcard,
    MissingRoot,
    MissingUser,
    Contradictory,
}
fn activation_server(
    failure: Failure,
    directory: Option<Arc<Mutex<Option<std::path::PathBuf>>>>,
) -> Server {
    let changed = Mutex::new(std::collections::BTreeSet::new());
    Server::new(move |headers, body| {
        let mut context = full_context(headers);
        let tenant = context["user"]["tenant_id"].as_str().unwrap().to_owned();
        if tenant != "system" {
            context["is_super_admin"] = json!(false);
            context["permissions"] = json!(["system:post:add", "system:post:list"]);
        }
        let mut changed = changed.lock().unwrap();
        let data = if headers.starts_with("POST /login ") {
            let body: Value = serde_json::from_slice(body).unwrap();
            assert_eq!(
                body["password"],
                if changed.contains(&tenant) {
                    CURRENT
                } else {
                    INITIAL
                }
            );
            json!({"access_token":if changed.contains(&tenant) {"current-token"} else {"initial-token"}})
        } else if headers.starts_with("GET /context ") {
            context["password_change_required"] = json!(!changed.contains(&tenant));
            if !changed.contains(&tenant) {
                context["is_super_admin"] = json!(false);
                context["permissions"] = json!([]);
                context["roles"] = json!([]);
            }
            context["user"]["password_change_required"] =
                context["password_change_required"].clone();
            if matches!(failure, Failure::MissingRoot) {
                context
                    .as_object_mut()
                    .unwrap()
                    .remove("password_change_required");
            }
            if matches!(failure, Failure::MissingUser) {
                context["user"]
                    .as_object_mut()
                    .unwrap()
                    .remove("password_change_required");
            }
            if matches!(failure, Failure::Contradictory) {
                context["user"]["password_change_required"] = json!(changed.contains(&tenant));
            }
            if changed.contains(&tenant) && matches!(failure, Failure::Wildcard) {
                context["is_super_admin"] = json!(false);
                context["permissions"] = json!(["*"]);
            }
            if changed.contains(&tenant) && matches!(failure, Failure::Subject) {
                context["user"]["id"] = json!("7");
            }
            if changed.contains(&tenant) && matches!(failure, Failure::Permission) {
                context["is_super_admin"] = json!(false);
                context["permissions"] = json!([]);
            }
            context
        } else if headers.starts_with("PUT /password ") {
            assert!(changed.insert(tenant), "改密不得重放");
            let body: Value = serde_json::from_slice(body).unwrap();
            assert_eq!(body, json!({"old_password":INITIAL,"new_password":CURRENT}));
            if let Some(shared) = &directory {
                let locked = shared.lock().unwrap();
                let directory = locked.as_ref().unwrap();
                assert!(directory.join("activation-00.intent.json").is_file());
                if matches!(failure, Failure::Confirmation) {
                    std::fs::write(directory.join("activation-00.confirmed.json"), b"preserve")
                        .unwrap();
                }
            }
            if matches!(failure, Failure::Disconnected) {
                return (0, Vec::new());
            }
            Value::Null
        } else {
            json!({})
        };
        (
            200,
            serde_json::to_vec(&json!({"code":200,"data":data})).unwrap(),
        )
    })
}

#[test]
fn fresh_admin_and_all_ten_owners_change_once_relogin_and_publish_private_receipts() {
    let server = activation_server(Failure::None, None);
    let fixture = InputsFixture::new(&server);
    runtime().block_on(async {
        for index in 0..11 {
            let mut client = fixture.client(index);
            client
                .activate(&fixture.inputs, index, &fixture.directory)
                .await
                .unwrap();
            assert_eq!(client.token().unwrap(), "current-token");
        }
    });
    assert_eq!(std::fs::read_dir(&fixture.directory).unwrap().count(), 33);
    let calls = server.requests.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(h, _)| h.starts_with("PUT /password "))
            .count(),
        11
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(h, _)| h.starts_with("POST /login "))
            .count(),
        22
    );
    for chunk in calls.chunks(7) {
        assert_eq!(chunk.len(), 7);
        assert!(chunk[2].0.starts_with("GET /context "));
        assert!(chunk[3].0.starts_with("PUT /password "));
        assert!(chunk[6].0.starts_with("GET /context "));
        assert!(!chunk[4].0.to_ascii_lowercase().contains("authorization:"));
        assert!(!chunk[5].0.to_ascii_lowercase().contains("authorization:"));
    }
    fixture.assert_private();
}

#[test]
fn committed_disconnect_and_confirmation_failure_leave_intent_and_block_all_reentry_requests() {
    for failure in [Failure::Disconnected, Failure::Confirmation] {
        let directory = Arc::new(Mutex::new(None));
        let server = activation_server(failure, Some(directory.clone()));
        let fixture = InputsFixture::new(&server);
        *directory.lock().unwrap() = Some(fixture.directory.clone());
        let mut client = fixture.client(0);
        runtime().block_on(async {
            assert!(
                client
                    .activate(&fixture.inputs, 0, &fixture.directory)
                    .await
                    .is_err()
            );
            let count = server.requests.lock().unwrap().len();
            let mut next = fixture.client(0);
            assert!(
                next.activate(&fixture.inputs, 0, &fixture.directory)
                    .await
                    .is_err()
            );
            assert_eq!(server.requests.lock().unwrap().len(), count);
        });
        assert!(
            fixture
                .directory
                .join("activation-00.intent.json")
                .is_file()
        );
        assert!(
            !fixture
                .directory
                .join("activation-00.completed.json")
                .exists()
        );
        assert_eq!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(h, _)| h.starts_with("PUT /password "))
                .count(),
            1
        );
        fixture.assert_private();
    }
}

#[test]
fn changed_subject_or_missing_permissions_cannot_publish_activation() {
    for failure in [Failure::Subject, Failure::Permission] {
        let server = activation_server(failure, None);
        let fixture = InputsFixture::new(&server);
        assert!(
            runtime()
                .block_on(
                    fixture
                        .client(0)
                        .activate(&fixture.inputs, 0, &fixture.directory)
                )
                .is_err()
        );
        assert!(
            fixture
                .directory
                .join("activation-00.confirmed.json")
                .exists()
        );
        assert!(
            !fixture
                .directory
                .join("activation-00.completed.json")
                .exists()
        );
        fixture.assert_private();
    }
}

#[test]
fn missing_equal_and_drifted_configuration_fail_before_any_http_request() {
    let server = activation_server(Failure::None, None);
    let fixture = InputsFixture::new(&server);
    std::fs::write(&fixture.credentials, b"changed-private-secret").unwrap();
    assert!(
        runtime()
            .block_on(
                fixture
                    .client(0)
                    .activate(&fixture.inputs, 0, &fixture.directory)
            )
            .is_err()
    );
    assert!(Inputs::read(&fixture.request, &fixture.inputs.root).is_err());
    for text in [
        "[bootstrap]\n",
        "[bootstrap]\nadmin_password='Current!Changed123'\n",
    ] {
        let config = fixture._temporary.path().join("config/app.test.toml");
        std::fs::write(&config, text).unwrap();
        let bytes = text.as_bytes();
        let mut binding = json!({"app_env":"test","directory":config.parent().unwrap(),
            "files":{"app.test.toml":{"bytes":bytes.len(),"sha256":super::super::model::sha(bytes)}}});
        binding["sha256"] = json!(super::super::model::hash(&binding).unwrap());
        assert!(
            crate::recovery::dataset_prepare::configuration::initial(
                &fixture.inputs.root,
                &binding,
                CURRENT,
                &mut Vec::new()
            )
            .is_err()
        );
    }
    assert!(server.requests.lock().unwrap().is_empty());
}

#[test]
fn ordinary_login_rejects_restricted_context_without_password_request() {
    let server = activation_server(Failure::None, None);
    let fixture = InputsFixture::new(&server);
    assert!(runtime().block_on(fixture.client(0).login()).is_err());
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(h, _)| h.starts_with("PUT /password "))
    );
    assert!(
        std::fs::read_dir(&fixture.directory)
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn wildcard_permissions_authorize_without_super_admin_flag() {
    let server = activation_server(Failure::Wildcard, None);
    let fixture = InputsFixture::new(&server);
    runtime()
        .block_on(
            fixture
                .client(0)
                .activate(&fixture.inputs, 0, &fixture.directory),
        )
        .unwrap();
}

#[test]
fn both_password_flags_are_required_and_must_agree_before_any_password_mutation() {
    for failure in [
        Failure::MissingRoot,
        Failure::MissingUser,
        Failure::Contradictory,
    ] {
        let server = activation_server(failure, None);
        let fixture = InputsFixture::new(&server);
        assert!(
            runtime()
                .block_on(
                    fixture
                        .client(0)
                        .activate(&fixture.inputs, 0, &fixture.directory)
                )
                .is_err()
        );
        assert!(runtime().block_on(fixture.client(0).login()).is_err());
        assert!(
            !server
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|(h, _)| h.starts_with("PUT /password "))
        );
        assert!(
            std::fs::read_dir(&fixture.directory)
                .unwrap()
                .next()
                .is_none()
        );
    }
}

#[test]
fn verify_only_current_credentials_do_not_require_bootstrap_or_create_activation_evidence() {
    let server = activation_server(Failure::None, None);
    let fixture = InputsFixture::new(&server);
    std::fs::remove_file(fixture._temporary.path().join("config/app.test.toml")).unwrap();
    let mut snapshots = Vec::new();
    assert_eq!(
        crate::recovery::dataset_prepare::configuration::current(
            &fixture.inputs.root,
            &fixture.inputs.plan["dataset"],
            &mut snapshots
        )
        .unwrap(),
        CURRENT
    );
    assert_eq!(snapshots.len(), 1);
    assert!(server.requests.lock().unwrap().is_empty());
    assert!(
        std::fs::read_dir(&fixture.directory)
            .unwrap()
            .next()
            .is_none()
    );
}
