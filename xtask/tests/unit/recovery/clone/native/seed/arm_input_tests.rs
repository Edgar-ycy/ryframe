use serde_json::json;

use super::*;
use crate::recovery::clone::native::lifecycle::lock::RunLock;

struct Fixture {
    backend: PathBuf,
    _root: tempfile::TempDir,
    run: PathBuf,
    source: Value,
    facts: Value,
    storage: Value,
    raw: Value,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let local = backend.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let root = tempfile::tempdir_in(local).unwrap();
        let run = root.path().join("run");
        let target = root.path().join("storage");
        for directory in [&run, &target] {
            fs::create_dir(directory).unwrap();
            document::write_new(&directory.join("manifest.json"), &json!({})).unwrap();
            state::initialize(directory).unwrap();
        }
        fs::create_dir(run.join("seed-runtime")).unwrap();
        let raw = file(&run.join("raw.json"), &json!({"retained":true}));
        let source = Self::finish(
            &run,
            "source-register",
            Some(&json!({
                "status":"seed_source_registered","remote_writes":0,"outbox_drained":true,
                "restore_qualified":false,"source_request":raw,"source_storage":raw,"generation_verified":raw
            })),
            None,
        );
        let facts = json!({"source_request":raw,"source_environment":raw,"generation_verified":raw,
            "registration":{"source_storage":raw}});
        let storage = json!({"path":target,"manifest":common::binding(&target.join("manifest.json")).unwrap(),
            "storage":{"request":raw,"storage":{"identity":{},"sha256":"fixture","process_receipt":raw,"launch_receipt":raw},
                "attempt":1,"data_directory":"fixture","api_url":"fixture","console_url":"fixture","restart_result":raw},
            "cache":{"request":raw,"runtime":raw,"redis":{},"owner":raw,"controller":raw,"attempt":1,
                "ready":raw,"restart_result":raw}});
        Self {
            backend,
            _root: root,
            run,
            source,
            facts,
            storage,
            raw,
        }
    }

    fn finish(run: &Path, mode: &str, result: Option<&Value>, failure: Option<&str>) -> Value {
        let (attempt, _) = state::begin(run, "seed-runtime", mode, &json!({})).unwrap();
        state::finish(run, attempt, result, failure).unwrap();
        result.map_or(Value::Null, |_| {
            common::binding(&state::result_path(run, attempt)).unwrap()
        })
    }

    fn request(&self, side: &str) -> Value {
        let directory = self._root.path().join(format!("target-{side}"));
        fs::create_dir(&directory).unwrap();
        let target_request = file(&directory.join("request.json"), &json!({"side":side}));
        let run = Path::new(self.storage["path"].as_str().unwrap());
        let registration = file(
            &directory.join("registration.json"),
            &json!({
                "request":target_request,"storage_run":{"path":run,"manifest":self.storage["manifest"],
                "state":common::binding(&run.join("state.json")).unwrap()}
            }),
        );
        json!({"format_version":1,"kind":"devex-clone-seed-arm-input","id":format!("arm-{side}"),
            "source_registration":self.source,"initialized":self.raw,"target_registration":registration,
            "target_initialized_files":self.raw,"target_environment":self.raw,"copy_directory":directory.join("copy"),
            "build_bridges":[]})
    }

    fn manifest(&self, request: &Value) -> Value {
        model::manifest(
            request,
            &self.facts,
            &self.storage,
            request["copy_directory"].as_str().unwrap(),
        )
    }

    fn result(&self, request: &Value, side: &str, output: &Path) -> Value {
        model::result(
            request,
            &self.facts,
            &self.storage,
            side,
            &common::binding(&output.join("request.json")).unwrap(),
            &common::binding(&output.join("manifest.json")).unwrap(),
        )
    }

    fn state(&self) -> Value {
        state::load(&self.run, true).unwrap()
    }
}

fn file(path: &Path, value: &Value) -> Value {
    document::write_new(path, value).unwrap();
    common::binding(path).unwrap()
}

fn adopt_base(fixture: &Fixture, base: &Value) -> (PathBuf, Vec<u8>) {
    Fixture::finish(&fixture.run, MODE, None, Some("Interrupted"));
    let original = output_path(&fixture.run, 2);
    fs::create_dir(&original).unwrap();
    let original_request = original.join("request.json");
    file(&original_request, base);
    let raw = fs::read(&original_request).unwrap();
    let bound = file(&fixture.run.join("arm-request.json"), base);
    let lock = RunLock::claim(&fixture.run).unwrap();
    let (attempt, row) = state::begin(&fixture.run, "seed-runtime", MODE, &json!({})).unwrap();
    state::bind_controller(&fixture.run, attempt, &lock.owner, &row).unwrap();
    let saved = fixture.state();
    let rows = state::attempts(&saved).unwrap();
    let failed = &rows[1];
    assert!(
        prove(
            &fixture.backend,
            &fixture.run,
            &saved,
            failed,
            &fixture.source,
            None
        )
        .is_err()
    );
    let active = Active::new(&fixture.run, attempt, &bound, base);
    prove(
        &fixture.backend,
        &fixture.run,
        &saved,
        failed,
        &fixture.source,
        Some(&active),
    )
    .unwrap();
    let selected = pending(&fixture.backend, &fixture.run, rows, &[], base).unwrap();
    let output = write_prefix(
        &fixture.backend,
        &fixture.run,
        attempt,
        selected,
        base,
        &fixture.manifest(base),
    )
    .unwrap();
    assert_eq!(output, original);
    state::finish(
        &fixture.run,
        attempt,
        Some(&fixture.result(base, "base", &output)),
        None,
    )
    .unwrap();
    lock.release().unwrap();
    (original, raw)
}

#[test]
fn adopted_original_prefix_remains_unchanged_and_second_side_uses_new_output() {
    let fixture = Fixture::new();
    let base = fixture.request("base");
    let (original, raw) = adopt_base(&fixture, &base);
    let original_request = original.join("request.json");
    let candidate = fixture.request("candidate");
    let saved = fixture.state();
    let rows = state::attempts(&saved).unwrap();
    prove(
        &fixture.backend,
        &fixture.run,
        &saved,
        &rows[1],
        &fixture.source,
        None,
    )
    .unwrap();
    let past = published(&fixture.backend, &fixture.run, rows, &candidate).unwrap();
    assert_eq!(past[0].side, "base");
    let selected = pending(&fixture.backend, &fixture.run, rows, &past, &candidate).unwrap();
    assert!(selected.is_none());
    let output = write_prefix(
        &fixture.backend,
        &fixture.run,
        4,
        selected,
        &candidate,
        &fixture.manifest(&candidate),
    )
    .unwrap();
    Fixture::finish(
        &fixture.run,
        MODE,
        Some(&fixture.result(&candidate, "candidate", &output)),
        None,
    );
    let saved = fixture.state();
    let both = published(
        &fixture.backend,
        &fixture.run,
        state::attempts(&saved).unwrap(),
        &candidate,
    )
    .unwrap();
    assert_eq!(
        both.iter()
            .map(|value| value.side.as_str())
            .collect::<Vec<_>>(),
        ["base", "candidate"]
    );
    assert_eq!(fs::read(original_request).unwrap(), raw);
    assert!(original.join("manifest.json").is_file());
}

#[test]
fn forged_passed_side_does_not_mark_a_failed_prefix_as_adopted() {
    let fixture = Fixture::new();
    let request = fixture.request("base");
    Fixture::finish(&fixture.run, MODE, None, Some("Interrupted"));
    let original = output_path(&fixture.run, 2);
    fs::create_dir(&original).unwrap();
    file(&original.join("request.json"), &request);
    file(&original.join("manifest.json"), &fixture.manifest(&request));
    Fixture::finish(
        &fixture.run,
        MODE,
        Some(&fixture.result(&request, "candidate", &original)),
        None,
    );
    let saved = fixture.state();
    let rows = state::attempts(&saved).unwrap();
    let error = prove(
        &fixture.backend,
        &fixture.run,
        &saved,
        &rows[1],
        &fixture.source,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("侧别"));
    assert_eq!(prefix(&fixture.backend, &original).unwrap(), Some(2));
}

#[test]
fn wrong_request_unknown_file_and_multiple_prefixes_keep_original_bytes() {
    let fixture = Fixture::new();
    let base = fixture.request("base");
    let candidate = fixture.request("candidate");
    Fixture::finish(&fixture.run, MODE, None, Some("Interrupted"));
    let first = output_path(&fixture.run, 2);
    fs::create_dir(&first).unwrap();
    file(&first.join("request.json"), &base);
    let saved = fixture.state();
    let rows = state::attempts(&saved).unwrap();
    assert!(pending(&fixture.backend, &fixture.run, rows, &[], &candidate).is_err());
    let unknown = first.join("other.json");
    file(&unknown, &json!({}));
    assert!(pending(&fixture.backend, &fixture.run, rows, &[], &base).is_err());
    fs::remove_file(unknown).unwrap();
    Fixture::finish(&fixture.run, MODE, None, Some("Interrupted"));
    let second = output_path(&fixture.run, 3);
    fs::create_dir(&second).unwrap();
    file(&second.join("request.json"), &base);
    let saved = fixture.state();
    let rows = state::attempts(&saved).unwrap();
    assert!(
        pending(&fixture.backend, &fixture.run, rows, &[], &base)
            .unwrap_err()
            .to_string()
            .contains("多个")
    );
    assert!(!first.join("manifest.json").exists());
    assert!(!second.join("manifest.json").exists());
}
