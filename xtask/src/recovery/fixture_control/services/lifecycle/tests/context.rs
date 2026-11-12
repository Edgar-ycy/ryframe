use super::*;

struct InputFixture {
    _temporary: tempfile::TempDir,
    _publication: crate::recovery::fixture_control::source_pair::execution::tests::Fixture,
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
        let publication =
            crate::recovery::fixture_control::source_pair::execution::tests::Fixture::new_in(&root);
        let (inputs, _) =
            crate::recovery::fixture_control::environment::protocol::tests::published_plan(
                &publication,
            );
        let review_value = read(&inputs.join("review.json")).unwrap();
        let execution =
            PathBuf::from(text(&review_value["scopes"]["seed"], "backend_dir").unwrap());
        let run = context::service_run(&review_value).unwrap();
        fs::create_dir_all(&run).unwrap();
        let review = root.join("review.json");
        let bootstrap = root.join("bootstrap.json");
        json_io::write_new(&review, &review_value).unwrap();
        let (code, planned) = crate::recovery::fixture_control::environment::protocol::execute(
            &serde_json::to_vec(&json!({"format_version":1,"operation":"plan","write":false,
                "arguments":{"backend":backend,"review":review,
                    "execution_source":publication.output.join("execution-source.json"),
                    "maintenance_build":inputs.join("maintenance_build.json"),"side":"seed"}}))
            .unwrap(),
        );
        assert_eq!(code, 0, "{planned}");
        let configuration = execution.join(".local-tests/context-configuration");
        fs::create_dir_all(&configuration).unwrap();
        fs::write(configuration.join("app.toml"), "# shared\n").unwrap();
        fs::write(
            configuration.join("app.test.toml"),
            "scope_id='prepared-test'\n",
        )
        .unwrap();
        let private = json!({"APP_ENV":"test"});
        json_io::write_new(
            &root.join("environment.json"),
            &json!({"environment":private}),
        )
        .unwrap();
        json_io::write_new(&bootstrap,&json!({
            "format_version":1,"kind":"reference-fixture-environment","status":"prepared","services_started":false,
            "remote_writes":0,"historical_data_used":false,"secret_files":{},
            "execution_backend":execution,"plan":planned["result"],"config_dir":configuration,
            "configuration_sha256":crate::recovery::fixture_control::artifact::runtime::configuration_digest(&configuration).unwrap(),
            "environment_sha256":crate::recovery::clone::native::digest::plan_hash(&private).unwrap()
        })).unwrap();
        let manifest_review = binding(&review).unwrap();
        #[cfg(windows)]
        let manifest_review = {
            let mut value = manifest_review;
            value["path"] = json!(review.to_str().unwrap().replace('\\', "/"));
            value
        };
        json_io::write_new(&run.join("manifest.json"),&json!({"format_version":1,"kind":"reference-fixture-service-run",
            "review":manifest_review,"bootstrap":binding(&bootstrap).unwrap(),"execution_backend":execution,
            "scope_id":review_value["services"]["rustfs"]["scope_id"],"data_directory_was_empty":true})).unwrap();
        state::initialize(&run).unwrap();
        Self {
            _temporary: temporary,
            _publication: publication,
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

fn input_spellings(path: &Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut spellings = vec![
            path.to_path_buf(),
            PathBuf::from(path.to_str().unwrap().replace('\\', "/")),
            PathBuf::from(format!(
                "{}/{}",
                path.parent().unwrap().display(),
                path.file_name().unwrap().to_str().unwrap()
            )),
        ];
        let text = path.to_str().unwrap();
        if text.as_bytes().get(1) == Some(&b':') && text.as_bytes()[0].is_ascii_alphabetic() {
            spellings.push(PathBuf::from(format!(
                "{}{}",
                text[..1].to_ascii_lowercase(),
                &text[1..]
            )));
        }
        spellings
    }
    #[cfg(not(windows))]
    {
        vec![path.to_path_buf()]
    }
}

#[test]
fn context_loading_and_guard_are_read_only_and_bind_every_document() {
    let input = InputFixture::new();
    let mut runtime = FakeRuntime::new();
    let state_before = fs::read(input.run.join("state.json")).unwrap();
    let value = input.load(&mut runtime).unwrap();
    assert!(runtime.events.is_empty());
    let documents = [
        &input.review,
        &input.bootstrap,
        &input.run.join("manifest.json"),
    ]
    .into_iter()
    .map(|path| (path.to_path_buf(), fs::read(path).unwrap()))
    .collect::<Vec<_>>();
    let context = Context::load(&input.backend, value.clone()).unwrap();
    for (review, bootstrap) in input_spellings(&input.review)
        .into_iter()
        .zip(input_spellings(&input.bootstrap))
    {
        let aliased = context::load(&mut runtime, &input.backend, &review, &bootstrap).unwrap();
        assert_eq!(
            serde_json::to_vec(&aliased["sources"]).unwrap(),
            serde_json::to_vec(&value["sources"]).unwrap()
        );
        for owner in input_spellings(&input.run.join("state.json")) {
            context.require_current_owner(&owner).unwrap();
        }
    }
    let owner_copy = input.review.parent().unwrap().join("state-copy.json");
    fs::copy(input.run.join("state.json"), &owner_copy).unwrap();
    assert!(context.require_current_owner(&owner_copy).is_err());
    assert!(
        context
            .require_current_owner(&input.backend.join("Cargo.toml"))
            .is_err()
    );
    let mut changed_state = state_before.clone();
    changed_state.push(b' ');
    fs::write(input.run.join("state.json"), changed_state).unwrap();
    assert!(
        context
            .require_current_owner(&input.run.join("state.json"))
            .is_err()
    );
    fs::write(input.run.join("state.json"), &state_before).unwrap();
    for (path, bytes) in documents {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    let native = query::dispatch(
        &input.backend,
        "service-context",
        &json!({
            "review":input.review,"environment":input.bootstrap
        }),
    )
    .unwrap();
    assert_eq!(native, value);
    assert_eq!(value["history"]["initial_complete"], false);
    assert_eq!(
        fs::read(input.run.join("state.json")).unwrap(),
        state_before
    );
    assert!(!input.run.join("run-control.guard").exists());
    assert!(!input.run.join("run.lock").exists());
    assert_eq!(fs::read_dir(input.run.join("results")).unwrap().count(), 0);
    assert!(runtime.events.is_empty());
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
        for selected in input_spellings(&input.review) {
            let loaded = context::load(&mut runtime, &input.backend, &selected, &input.bootstrap);
            assert_eq!(loaded.is_ok(), accepted);
            if let Ok(value) = loaded {
                assert_eq!(value["sources"]["review"], binding(&input.review).unwrap());
            }
        }
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

    for (failed, _case_alias) in [(false, false), (true, false), (false, true)] {
        let mut input = InputFixture::new();
        let mut review = read(&input.review).unwrap();
        review["preflight"] = json!({"supersedes":binding(&input.review).unwrap()});
        input.review = input
            .review
            .parent()
            .unwrap()
            .join("registered-successor.json");
        json_io::write_new(&input.review, &review).unwrap();
        let registered = binding(&input.review).unwrap();
        #[cfg(windows)]
        let registered = {
            let mut registered = registered;
            registered["path"] = if _case_alias {
                json!(input.review.with_file_name("REGISTERED-SUCCESSOR.JSON"))
            } else {
                json!(input.review.to_str().unwrap().replace('\\', "/"))
            };
            registered
        };
        let (number, _) = state::begin(
            &input.run,
            "storage-target",
            "initial",
            &json!({"review":registered}),
        )
        .unwrap();
        if failed {
            state::finish(&input.run, number, None, Some("FixtureFailure")).unwrap();
        } else {
            state::finish(&input.run, number, Some(&json!({"fixture":true})), None).unwrap();
        }
        let before = fs::read(input.run.join("state.json")).unwrap();
        for selected in input_spellings(&input.review) {
            let loaded = context::load(
                &mut FakeRuntime::new(),
                &input.backend,
                &selected,
                &input.bootstrap,
            );
            match loaded {
                Ok(value) => assert_eq!(value["sources"]["review"], registered),
                // 通用binding的既有规范限制可以拒绝，不能跳过历史绑定后静默重签。
                #[cfg(windows)]
                Err(error) if _case_alias => assert!(
                    error.to_string().contains("必须使用规范绝对路径")
                        || !Path::new(text(&registered, "path").unwrap())
                            .try_exists()
                            .unwrap()
                ),
                Err(error) => panic!("登记审阅未被保留：{error}"),
            }
        }
        let mut rejected = FakeRuntime::new();
        rejected.fail = Some("review-successor".into());
        assert!(input.load(&mut rejected).is_err());
        assert_eq!(fs::read(input.run.join("state.json")).unwrap(), before);
    }
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
fn prepared_environment_requires_exact_digest_credentials_and_no_service_side_effects() {
    let input = InputFixture::new();
    let mut receipt = read(&input.bootstrap).unwrap();
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
        ("plan", json!({"review": receipt["plan"]["review"]})),
        ("config_dir", json!(input.bootstrap.parent().unwrap())),
    ] {
        let mut changed = receipt.clone();
        changed[key] = invalid;
        json_io::write_replace(&input.bootstrap, &changed).unwrap();
        let error = prepared::load(&input.backend, &input.bootstrap, &json!([])).unwrap_err();
        if key == "plan" {
            assert!(error.to_string().contains("缺少执行源码发布绑定"));
        }
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
    let input = InputFixture::new();
    let copied_review = input.review.parent().unwrap().join("copied-review.json");
    let state_before = fs::read(input.run.join("state.json")).unwrap();
    let copied_bootstrap = input
        .bootstrap
        .parent()
        .unwrap()
        .join("copied-bootstrap.json");
    fs::copy(&input.review, &copied_review).unwrap();
    fs::copy(&input.bootstrap, &copied_bootstrap).unwrap();
    for (review, bootstrap) in [
        (&copied_review, &input.bootstrap),
        (&input.review, &copied_bootstrap),
    ] {
        assert!(context::load(&mut FakeRuntime::new(), &input.backend, review, bootstrap).is_err());
    }
    let alias_root = tempfile::tempdir_in(input.backend.join(".local-tests")).unwrap();
    let linked = alias_root.path().join("linked");
    create_directory_link(input.review.parent().unwrap(), &linked);
    assert!(
        context::load(
            &mut FakeRuntime::new(),
            &input.backend,
            &linked.join("review.json"),
            &input.bootstrap
        )
        .is_err()
    );
    assert!(
        context::load(
            &mut FakeRuntime::new(),
            &input.backend,
            &input.review,
            &linked.join("bootstrap.json")
        )
        .is_err()
    );
    let value = input.load(&mut FakeRuntime::new()).unwrap();
    let context = Context::load(&input.backend, value.clone()).unwrap();
    let linked_owner = linked
        .join(
            input
                .run
                .strip_prefix(input.review.parent().unwrap())
                .unwrap(),
        )
        .join("state.json");
    assert!(context.require_current_owner(&linked_owner).is_err());
    let mut changed = value;
    changed["sources"]["state_before"]["path"] = json!(linked_owner);
    assert!(
        Context {
            backend: input.backend.clone(),
            run: input.run.clone(),
            value: changed
        }
        .require_current_owner(&input.run.join("state.json"))
        .is_err()
    );
    #[cfg(windows)]
    fs::remove_dir(&linked).unwrap();
    #[cfg(unix)]
    fs::remove_file(&linked).unwrap();
    assert_eq!(
        fs::read(input.run.join("state.json")).unwrap(),
        state_before
    );
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success());
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
