use super::*;

struct Fixture {
    backend: PathBuf,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let local = backend.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::tempdir_in(local).unwrap();
        document::write_new(&directory.path().join("manifest.json"), &json!({})).unwrap();
        state::initialize(directory.path()).unwrap();
        Self { backend, directory }
    }

    fn run(&self) -> &Path {
        self.directory.path()
    }

    fn preflight(&self, mode: &str) -> Result<()> {
        Runner.preflight(&StageRequest {
            backend: &self.backend,
            run_dir: self.run(),
            stage: "seed-runtime",
            mode,
            attempt: None,
        })
    }

    fn finish(&self, mode: &str, result: Option<&Value>, failure: Option<&str>) -> u64 {
        let (number, _) = state::begin(self.run(), "seed-runtime", mode, &json!({})).unwrap();
        state::finish(self.run(), number, result, failure).unwrap();
        number
    }

    fn prior(&self) -> Vec<Value> {
        let saved = state::load(self.run(), true).unwrap();
        export_rows(state::attempts(&saved).unwrap())
            .into_iter()
            .cloned()
            .collect()
    }
}

#[test]
fn partial_or_marked_shared_candidate_does_not_open_an_attempt_or_allow_reexport() {
    let fixture = Fixture::new();
    fixture.preflight(EXPORT).unwrap();
    assert_eq!(fixture.finish(EXPORT, None, Some("Interrupted")), 1);
    let state_path = fixture.run().join("state.json");
    let history = fs::read(&state_path).unwrap();
    assert!(
        fixture
            .preflight(EXPORT)
            .unwrap_err()
            .to_string()
            .contains("只允许一次")
    );
    assert!(
        fixture
            .preflight(RECONCILE)
            .unwrap_err()
            .to_string()
            .contains("缺少完整")
    );
    let original = fixture.run().join("e0001");
    fs::create_dir(&original).unwrap();
    document::write_new(&original.join("export.json"), &json!({})).unwrap();
    let failure = original.join("failure.json");
    document::write_new(&failure, &json!({"status":"source_export_failed"})).unwrap();
    assert!(
        fixture
            .preflight(RECONCILE)
            .unwrap_err()
            .to_string()
            .contains("失败标记")
    );
    assert_eq!(fs::read(&state_path).unwrap(), history);
    assert!(!fixture.run().join("controller-0002.json").exists());
}

#[test]
fn adoption_keeps_original_receipts_and_only_accepts_identical_prior_results() {
    let fixture = Fixture::new();
    fixture.finish(EXPORT, None, Some("Interrupted"));
    let original = fixture.run().join("e0001");
    fs::create_dir(&original).unwrap();
    let export = original.join("export.json");
    document::write_new(&export, &json!({"request":"original"})).unwrap();
    let raw = fs::read(&export).unwrap();
    let expected = json!({"status":"seed_source_export_published",
        "origin_attempt":1,"export":common::binding(&export).unwrap()});
    fixture.finish(RECONCILE, None, Some("TemporaryVerificationFailure"));
    fixture.finish(
        RECONCILE,
        Some(&expected),
        Some("OuterPublicationInterrupted"),
    );
    fixture.preflight(RECONCILE).unwrap();
    let rows = fixture.prior();
    let prior = export_rows(&rows);
    let origin = origin(&prior, RECONCILE).unwrap().unwrap();
    assert_eq!(number(origin).unwrap(), 1);
    assert_eq!(
        candidate(&fixture.backend, fixture.run(), origin).unwrap(),
        export
    );
    check_prior_results(&fixture.backend, fixture.run(), &prior, &expected).unwrap();
    let mut changed = expected.clone();
    changed["export"]["bytes"] = json!(0);
    assert!(check_prior_results(&fixture.backend, fixture.run(), &prior, &changed).is_err());
    assert!(
        read_seal(&fixture.backend, &seal_path(fixture.run(), 1))
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read(&export).unwrap(), raw);
    fixture.finish(RECONCILE, Some(&expected), None);
    assert!(fixture.preflight(RECONCILE).is_err());
    assert!(fixture.preflight(EXPORT).is_err());
}

#[test]
fn shared_export_execution_requires_the_same_current_controller() {
    let fixture = Fixture::new();
    let request = StageRequest {
        backend: &fixture.backend,
        run_dir: fixture.run(),
        stage: "seed-runtime",
        mode: EXPORT,
        attempt: Some(1),
    };
    let run_lock = lock::RunLock::claim(fixture.run()).unwrap();
    let (number, row) = state::begin(fixture.run(), "seed-runtime", EXPORT, &json!({})).unwrap();
    assert!(active(&request, std::slice::from_ref(&row)).is_err());
    state::bind_controller(fixture.run(), number, &run_lock.owner, &row).unwrap();
    assert_eq!(active(&request, std::slice::from_ref(&row)).unwrap(), &row);
    run_lock.release().unwrap();
    assert!(active(&request, std::slice::from_ref(&row)).is_err());
}

#[test]
fn final_publication_rejects_changed_seal_and_saved_generation_proof() {
    let fixture = Fixture::new();
    let intent = intent_path(fixture.run(), 1);
    document::write_new(&intent, &json!({"attempt":1})).unwrap();
    let output = fixture.run().join("e0001");
    fs::create_dir(&output).unwrap();
    let export = output.join("export.json");
    let generation = output.join("generation-before.json");
    let saved_generation = json!({"source":"original"});
    document::write_new(&export, &json!({"status":"source_export_captured"})).unwrap();
    document::write_new(&generation, &saved_generation).unwrap();
    let bound = common::binding(&generation).unwrap();
    let proof_path = bound["path"].as_str().unwrap();
    let result = json!({"origin_intent":common::binding(&intent).unwrap(),
        "export":common::binding(&export).unwrap()});
    let seal = seal_path(fixture.run(), 1);
    let publication = || Publication {
        origin: 1,
        result: result.clone(),
        seal: None,
        evidence: frozen::FrozenExport {
            request: Value::Null,
            generation: saved_generation.clone(),
            inventory: Value::Null,
            captures: Default::default(),
            proof_files: json!({(proof_path):{"bytes":bound["bytes"],"sha256":bound["sha256"]}}),
        },
    };
    assert_eq!(
        publication()
            .close(&fixture.backend, fixture.run())
            .unwrap(),
        result
    );
    document::write_new(&seal, &json!({"different":"seal"})).unwrap();
    assert!(
        publication()
            .close(&fixture.backend, fixture.run())
            .unwrap_err()
            .to_string()
            .contains("封存")
    );
    fs::remove_file(seal).unwrap();
    fs::write(generation, b"{\"source\":\"changed\"}").unwrap();
    assert!(
        publication()
            .close(&fixture.backend, fixture.run())
            .unwrap_err()
            .to_string()
            .contains("证明")
    );
}
