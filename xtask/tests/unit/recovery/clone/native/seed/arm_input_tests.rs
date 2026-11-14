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

fn expected_arm_manifest(fixture: &Fixture, request: &Value, source: &Value) -> Value {
    json!({
        "format_version":1,"kind":"devex-clone-run","id":"arm-base","source_request":source["source_request"],
        "source_export":null,"initialized":fixture.raw,"source_environment":source["source_environment"],
        "target_environment":fixture.raw,"copy_directory":request["copy_directory"],"copy_stage":"seed_to_arm",
        "build_bridges":[],"source_registration":fixture.source,
        "target_registration":request["target_registration"],"target_initialized_files":fixture.raw,
        "target_storage_run":fixture.storage
    })
}

#[test]
fn arm_model_derives_exact_ordinary_and_successor_documents() {
    let fixture = Fixture::new();
    let mut request = fixture.request("base");
    let input = fixture._root.path();
    let descriptor = |name: &str| file(&input.join(format!("{name}.json")), &json!({"kind":name}));
    let source_request = descriptor("source-request");
    let source_environment = descriptor("source-environment");
    let source_storage = descriptor("source-storage");
    let generation_verified = descriptor("generation-verified");
    let source = json!({
        "source_request":source_request,"source_environment":source_environment,
        "generation_verified":generation_verified,"registration":{"source_storage":source_storage}
    });
    let request_binding = descriptor("arm-request");
    let manifest_binding = descriptor("arm-manifest");
    let copy = request["copy_directory"].as_str().unwrap().to_owned();
    let mut expected_manifest = expected_arm_manifest(&fixture, &request, &source);
    let mut expected_result = json!({
        "status":"seed_arm_input_published","request":request_binding,"manifest":manifest_binding,
        "source_registration":fixture.source,"source_request":source_request,"source_storage":source_storage,
        "generation_verified":generation_verified,"initialized":fixture.raw,"target_environment":fixture.raw,
        "target_registration":request["target_registration"],"target_initialized_files":fixture.raw,
        "target_storage_run":fixture.storage,"target_side":"base","remote_writes":0,
        "outbox_drained":true,"restore_qualified":false
    });
    let project_manifest = |value: &Value| model::manifest(value, &source, &fixture.storage, &copy);
    let project_result = |value: &Value| {
        model::result(
            value,
            &source,
            &fixture.storage,
            "base",
            &request_binding,
            &manifest_binding,
        )
    };
    assert!(!model::request(&request).unwrap());
    assert_eq!(project_manifest(&request), expected_manifest);
    assert_eq!(project_result(&request), expected_result);
    model::check_result(&expected_result, &request).unwrap();
    for field in [
        "source_request",
        "source_environment",
        "source_storage",
        "review_successor",
    ] {
        let mut wrong = request.clone();
        wrong[field] = source_request.clone();
        let error = model::request(&wrong).unwrap_err().to_string();
        assert!(error.contains("字段缺失或包含未知字段"), "{field}: {error}");
    }

    request["kind"] = json!("devex-clone-seed-successor-arm-input");
    assert!(model::request(&request).is_err());
    for (field, name) in [
        ("review_successor", "successor"),
        ("source_export", "shared-export"),
        ("source_export_result", "export-result"),
        ("source_generation", "stopped-generation"),
        ("source_rebind", "storage-rebind"),
    ] {
        let binding = descriptor(name);
        request[field] = binding.clone();
        expected_manifest[field] = binding.clone();
        expected_result[field] = binding;
    }
    assert!(model::request(&request).unwrap());
    for field in [
        "review_successor",
        "source_export",
        "source_export_result",
        "source_generation",
    ] {
        let mut missing = request.clone();
        missing.as_object_mut().unwrap().remove(field);
        let error = model::request(&missing).unwrap_err().to_string();
        assert!(error.contains("字段缺失或包含未知字段"), "{field}: {error}");
    }
    assert_eq!(project_manifest(&request), expected_manifest);
    assert_eq!(project_result(&request), expected_result);
    model::check_fields(&expected_manifest, &request).unwrap();
    model::check_result(&expected_result, &request).unwrap();
    for replacement in [None, Some(source_storage)] {
        let mut manifest = expected_manifest.clone();
        let mut result = expected_result.clone();
        for value in [&mut manifest, &mut result] {
            if let Some(binding) = &replacement {
                value["source_rebind"] = binding.clone();
            } else {
                value.as_object_mut().unwrap().remove("source_rebind");
            }
        }
        assert!(model::check_fields(&manifest, &request).is_err());
        assert!(model::check_result(&result, &request).is_err());
    }
}

#[test]
fn published_arm_accepts_an_absent_or_existing_copy_directory_but_rejects_a_file() {
    let fixture = Fixture::new();
    let request = fixture.request("base");
    let copy = Path::new(request["copy_directory"].as_str().unwrap());
    assert_eq!(
        PathBuf::from(super::super::copy_directory(&fixture.backend, &request).unwrap()),
        copy
    );
    let output = write_prefix(
        &fixture.backend,
        &fixture.run,
        2,
        None,
        &request,
        &fixture.manifest(&request),
    )
    .unwrap();
    let result = fixture.result(&request, "base", &output);
    Fixture::finish(&fixture.run, MODE, Some(&result), None);
    let paths = [
        output.join("request.json"),
        output.join("manifest.json"),
        fixture.run.join("state.json"),
        state::result_path(&fixture.run, 2),
    ];
    let before = paths.each_ref().map(|path| fs::read(path).unwrap());
    let read_published = || {
        let saved = fixture.state();
        published(
            &fixture.backend,
            &fixture.run,
            state::attempts(&saved).unwrap(),
            &request,
        )
    };
    assert!(!copy.exists());
    assert_eq!(read_published().unwrap()[0].side, "base");
    assert!(!copy.exists());
    fs::create_dir(copy).unwrap();
    assert_eq!(read_published().unwrap()[0].side, "base");
    assert!(super::super::copy_directory(&fixture.backend, &request).is_err());
    fs::remove_dir(copy).unwrap();
    fs::write(copy, b"original copy marker").unwrap();
    let error = read_published().err().unwrap().to_string();
    assert!(
        error.contains("复制目录无效") && error.contains("必须是目录"),
        "{error}"
    );
    assert_eq!(fs::read(copy).unwrap(), b"original copy marker");
    assert_eq!(paths.each_ref().map(|path| fs::read(path).unwrap()), before);
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
