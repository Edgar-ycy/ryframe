use super::*;

struct InputFixture {
    _temporary: tempfile::TempDir,
    backend: PathBuf,
    review: PathBuf,
    bootstrap: PathBuf,
    run: PathBuf,
}
impl InputFixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let temporary = tempfile::tempdir_in(backend.join(".local-tests")).unwrap();
        let root = crate::recovery::fixture_control::portable_path(
            &fs::canonicalize(temporary.path()).unwrap(),
        );
        let execution = root.join("business");
        let future = execution
            .join(".local-tests")
            .join("reference-fixture")
            .join("run");
        let run = future.join("service-run");
        fs::create_dir_all(&run).unwrap();
        let review = root.join("review.json");
        let bootstrap = root.join("bootstrap.json");
        json_io::write_new(&execution.join("Cargo.toml"), &json!({"fixture":true})).unwrap();
        json_io::write_new(
            &review,
            &json!({"future_root":future,"scopes":{"seed":{"backend_dir":execution}},
            "services":{"rustfs":{"scope_id":"context-test"}}}),
        )
        .unwrap();
        json_io::write_new(&bootstrap,&json!({"kind":"reference-fixture-environment","status":"prepared","services_started":false,
            "remote_writes":0,"execution_backend":execution,"plan":{"review":binding(&review).unwrap()}})).unwrap();
        json_io::write_new(
            &root.join("environment.json"),
            &json!({"environment":{"APP_ENV":"test"}}),
        )
        .unwrap();
        json_io::write_new(&run.join("manifest.json"),&json!({"format_version":1,"kind":"reference-fixture-service-run",
            "review":binding(&review).unwrap(),"bootstrap":binding(&bootstrap).unwrap(),"execution_backend":execution,
            "scope_id":"context-test","data_directory_was_empty":true})).unwrap();
        state::initialize(&run).unwrap();
        Self {
            _temporary: temporary,
            backend,
            review,
            bootstrap,
            run,
        }
    }
    fn load(&self, runtime: &mut dyn Runtime) -> Result<Value> {
        context::load(runtime, &self.backend, &self.review, &self.bootstrap)
    }
}

#[test]
fn context_loading_and_guard_are_read_only_and_bind_every_document() {
    let input = InputFixture::new();
    let mut runtime = FakeRuntime::new();
    let state_before = fs::read(input.run.join("state.json")).unwrap();
    let value = input.load(&mut runtime).unwrap();
    assert!(runtime.events.is_empty());
    assert_eq!(value["history"]["initial_complete"], false);
    assert_eq!(
        fs::read(input.run.join("state.json")).unwrap(),
        state_before
    );
    assert!(!input.run.join("run-control.guard").exists());
    for key in ["review", "bootstrap", "manifest", "environment"] {
        let path = Path::new(text(&value["sources"][key], "path").unwrap());
        let before = fs::read(path).unwrap();
        fs::write(path, b"{}").unwrap();
        assert!(context::guarded(&input.backend, &value, None).is_err());
        fs::write(path, before).unwrap();
    }
}

#[test]
fn context_rejects_unbound_review_and_accepts_only_verified_successor() {
    for accepted in [true, false] {
        let mut input = InputFixture::new();
        let mut runtime = FakeRuntime::new();
        let before = binding(&input.review).unwrap();
        let mut review = read(&input.review).unwrap();
        review["preflight"] = json!({"supersedes":before});
        input.review = input.review.parent().unwrap().join("successor.json");
        json_io::write_new(&input.review, &review).unwrap();
        if !accepted {
            runtime.fail = Some("review-successor".into());
        }
        assert_eq!(input.load(&mut runtime).is_ok(), accepted);
        assert!(
            runtime
                .events
                .iter()
                .any(|operation| operation == "review-successor")
        );
    }
    let input = InputFixture::new();
    let mut review = read(&input.review).unwrap();
    review["unreviewed"] = json!(true);
    json_io::write_replace(&input.review, &review).unwrap();
    assert!(input.load(&mut FakeRuntime::new()).is_err());
}

#[test]
fn initial_transition_validates_context_but_never_reads_live_services() {
    let mut runtime = FakeRuntime::new();
    let backend = runtime.fixture.backend.clone();
    let run = runtime.fixture.run.clone();
    assert_eq!(
        projection::transition(&mut runtime, &backend, &run, &json!({})).unwrap(),
        json!([null, null])
    );
    assert!(
        !runtime
            .events
            .iter()
            .any(|operation| ["registered", "observe", "identity"].contains(&operation.as_str()))
    );
}

#[test]
fn transition_uses_current_review_and_registered_historical_generations() {
    let mut runtime = FakeRuntime::new();
    runtime.execute("close", None).unwrap();
    runtime.execute("restart", Some(runtime.owner())).unwrap();
    let value = runtime.fixture.state();
    let outer = read(Path::new(
        text(&value["attempts"][4]["result"], "path").unwrap(),
    ))
    .unwrap();
    let generation = read(Path::new(text(&outer["generation"], "path").unwrap())).unwrap();
    runtime.fixture.services["storage"] = generation["rustfs"]["storage"].clone();
    runtime.fixture.services["runtime"] = generation["redis"]["runtime"].clone();
    runtime.fixture.services["requests"]["rustfs"]["api_url"] = json!("http://127.0.0.1:29200");
    runtime.fixture.services["requests"]["rustfs"]["console_url"] = json!("http://127.0.0.1:29201");
    let expected = runtime.fixture.services["origin"].clone();
    let backend = runtime.fixture.backend.clone();
    let run = runtime.fixture.run.clone();
    let result = projection::transition(&mut runtime, &backend, &run, &expected).unwrap();
    assert_eq!(result[0]["generation"], value["attempts"][4]["result"]);
    assert!(
        projection::transition(
            &mut runtime,
            &backend,
            &run,
            &json!({"storage":{"identity":"foreign"}})
        )
        .is_err()
    );
    assert_eq!(runtime.fixture.state(), value);
}

#[test]
fn preparation_projection_requires_exact_prefix_and_direct_credentials() {
    let backend = crate::workspace::root_dir();
    let state = json!({"attempts":[{"stage":"storage-target","mode":"initial","status":"passed"},
        {"stage":"cache-target","mode":"initial","status":"passed"}]});
    assert!(query::dispatch(&backend, "service-initial-prefix", &json!({"state":state})).is_ok());
    for invalid in [
        json!({"attempts":[]}),
        json!({"attempts":[{"stage":"cache-target","mode":"initial","status":"passed"}]}),
    ] {
        assert!(
            query::dispatch(
                &backend,
                "service-initial-prefix",
                &json!({"state":invalid})
            )
            .is_err()
        );
    }
    let args = json!({"review":{"scopes":{"seed":{"objects":{"endpoint":"http://127.0.0.1:29200","region":"us-east-1"}}}},
        "private":{"object_storage":{"access_key":"file-access","secret_key":"file-secret"}}});
    assert_eq!(
        query::dispatch(&backend, "service-bucket-configuration", &args).unwrap()["secret_key"],
        "file-secret"
    );
    for key in ["access_key", "secret_key"] {
        let mut invalid = args.clone();
        invalid["private"]["object_storage"][key] = Value::Null;
        let error =
            query::dispatch(&backend, "service-bucket-configuration", &invalid).unwrap_err();
        assert!(!error.to_string().contains("file-secret"));
    }
}

#[test]
fn prepared_environment_requires_exact_digest_credentials_and_no_service_side_effects() {
    let input = InputFixture::new();
    let mut receipt = read(&input.bootstrap).unwrap();
    receipt["format_version"] = json!(1);
    receipt["historical_data_used"] = json!(false);
    receipt["secret_files"] = json!({});
    receipt["environment_sha256"] = json!(
        crate::recovery::clone::native::digest::plan_hash(&json!({"APP_ENV":"test"})).unwrap()
    );
    json_io::write_replace(&input.bootstrap, &receipt).unwrap();
    let before = fs::read(&input.bootstrap).unwrap();
    assert!(prepared::load(&input.backend, &input.bootstrap, &json!([])).is_ok());
    assert_eq!(fs::read(&input.bootstrap).unwrap(), before);
    assert!(!input.run.join("run-control.guard").exists());
    for (key, invalid) in [
        ("services_started", json!(true)),
        ("remote_writes", json!(1)),
        ("historical_data_used", json!(true)),
        ("environment_sha256", json!("0".repeat(64))),
        ("secret_files", json!({"foreign.txt":{}})),
    ] {
        let mut changed = receipt.clone();
        changed[key] = invalid;
        json_io::write_replace(&input.bootstrap, &changed).unwrap();
        assert!(
            prepared::load(&input.backend, &input.bootstrap, &json!([])).is_err(),
            "{key}"
        );
    }
    json_io::write_replace(&input.bootstrap, &receipt).unwrap();
    let secret = input.bootstrap.parent().unwrap().join("secret.txt");
    fs::write(&secret, b"private-marker").unwrap();
    receipt["secret_files"] = json!({"secret.txt":binding(&secret).unwrap()});
    json_io::write_replace(&input.bootstrap, &receipt).unwrap();
    assert!(prepared::load(&input.backend, &input.bootstrap, &json!(["secret.txt"])).is_ok());
    fs::write(&secret, b"changed-private-marker").unwrap();
    assert!(prepared::load(&input.backend, &input.bootstrap, &json!(["secret.txt"])).is_err());
}

#[test]
fn service_run_and_documents_reject_escape_and_unknown_protocol_fields() {
    let backend = crate::workspace::root_dir();
    let review = json!({"future_root":backend.join(".local-tests/reference-fixture/run"),
        "scopes":{"seed":{"backend_dir":backend}}});
    assert_eq!(
        context::service_run(&review).unwrap(),
        backend.join(".local-tests/reference-fixture/run/service-run")
    );
    let mut escaped = review;
    escaped["future_root"] = json!(backend.join(".local-tests/foreign/run"));
    assert!(context::service_run(&escaped).is_err());
    assert!(
        query::dispatch(
            &backend,
            "service-document",
            &json!({"path":backend.join("Cargo.toml")})
        )
        .is_err()
    );
    assert!(
        query::dispatch(
            &backend,
            "service-run",
            &json!({"review":escaped,"secret":"private-marker"})
        )
        .is_err()
    );
}

fn use_active_services(runtime: &mut FakeRuntime) -> Value {
    let history = runtime.fixture.history();
    let outer = read(Path::new(
        text(&history["active_generation"], "path").unwrap(),
    ))
    .unwrap();
    let generation = read(Path::new(text(&outer["generation"], "path").unwrap())).unwrap();
    let storage = generation["rustfs"]["storage"].clone();
    let tree = read(Path::new(text(&storage["tree"], "path").unwrap())).unwrap();
    for role in ["rustfs", "redis"] {
        runtime.fixture.services["requests"][role] = read(Path::new(
            text(&generation[role]["request"], "path").unwrap(),
        ))
        .unwrap();
    }
    runtime.fixture.services["tree"] = tree;
    runtime.fixture.services["storage"] = storage.clone();
    runtime.fixture.services["runtime"] = generation["redis"]["runtime"].clone();
    runtime.fixture.services["evidence"] = json!({"rustfs_tree":storage["tree"]});
    json!({"storage":{"identity":storage["identity"],"sha256":storage["sha256"],"process_receipt":storage["process_receipt"],
        "launch_receipt":storage["launch_receipt"]},"redis":generation["redis"]["runtime"]["redis"]})
}

#[test]
fn transition_accepts_a_previous_registered_generation_and_rejects_its_tampered_evidence() {
    let mut runtime = FakeRuntime::new();
    runtime.fixture.services["requests"]["rustfs"]["api_url"] = json!("http://127.0.0.1:29200");
    runtime.fixture.services["requests"]["rustfs"]["console_url"] = json!("http://127.0.0.1:29201");
    json_io::write_replace(
        &runtime.fixture.run.join("rustfs/request.json"),
        &runtime.fixture.services["requests"]["rustfs"],
    )
    .unwrap();
    runtime.execute("close", None).unwrap();
    runtime.execute("restart", Some(runtime.owner())).unwrap();
    let previous = use_active_services(&mut runtime);
    runtime.execute("close", None).unwrap();
    runtime.execute("restart", Some(runtime.owner())).unwrap();
    use_active_services(&mut runtime);
    let backend = runtime.fixture.backend.clone();
    let run = runtime.fixture.run.clone();
    let result = projection::transition(&mut runtime, &backend, &run, &previous).unwrap();
    assert_eq!(
        result[0]["generation"],
        runtime.fixture.history()["active_generation"]
    );
    let path = PathBuf::from(text(&previous["storage"]["process_receipt"], "path").unwrap());
    fs::write(path, b"{}").unwrap();
    assert!(projection::transition(&mut runtime, &backend, &run, &previous).is_err());
}
