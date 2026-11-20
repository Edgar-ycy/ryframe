use std::{fs, path::Path};

use super::{
    finish_cases::{
        completion_count, database_writes, four_database_declaration, run_published_copy,
    },
    resume_orphan::bind_copy_files,
    *,
};
use crate::recovery::clone::native::{
    copy_ledger::inspect,
    copy_transfer::Runner,
    lifecycle::{
        stage::{self, StageRequest, StageRunner},
        state,
    },
};

#[test]
fn changed_source_environment_rejects_reconcile_without_rebinding_original_copy() {
    let mut fixture = Fixture::new();
    let (run_dir, manifest) = fixture.register_copy_run().unwrap();
    let mut declaration = four_database_declaration(1);
    declaration["format_version"] = json!(1);
    declaration["copy_id"] = manifest["id"].clone();
    declaration["copy_stage"] = manifest["copy_stage"].clone();
    declaration["pending_target_actions"] = json!([]);
    let prepared = bind_copy_files(&fixture, &manifest, &declaration).unwrap();
    let plan = Plan::from_verified(&prepared.plan).unwrap();
    let mut io = FixtureIo::new(&plan);
    database_writes(&mut io, &plan.databases[..2]);
    let first = &plan.databases[0].id;
    let after = io.bases[first]["after"].clone();
    io.stop_at = Some(first.clone());
    io.committed_write_image = Some(after.clone());
    let error = run_published_copy(&fixture, &prepared, &plan, Mode::New, &mut io).unwrap_err();
    assert_eq!(error.to_string(), "复制进程结束，资源写入结果未知");
    assert_eq!(io.writes.as_slice(), std::slice::from_ref(first));
    assert_eq!(io.current[first], after);

    let ledger = fixture.copy.join("ledger");
    let inspect_ledger = || {
        inspect(
            &fixture.backend,
            &ledger,
            prepared.record["plan_sha256"].as_str().unwrap(),
            prepared.record["generation_sha256"].as_str().unwrap(),
        )
        .unwrap()
    };
    let unknown = inspect_ledger();
    assert_eq!(unknown["status"], "needs_reconciliation");
    assert_eq!(unknown["steps"].as_object().unwrap().len(), 1);
    assert_eq!(unknown["steps"][first]["phase"], "unknown");
    assert_eq!(unknown["steps"][first]["attempt"], 1);
    let failure = document::read(&fixture.copy.join("failure.json")).unwrap();
    assert_eq!(failure["phase"], "transfer");
    assert_eq!(failure["status"], "needs_reconciliation");

    let paths = original_paths(&fixture, &run_dir, &manifest);
    let original_bytes = read_files(&paths);
    let parent = run_dir.parent().unwrap();
    let directories = [
        parent.to_owned(),
        run_dir.clone(),
        fixture.copy.clone(),
        ledger.clone(),
        parent.join("source"),
        parent.join("target"),
    ];
    let original_members = directories.each_ref().map(|path| directory_entries(path));
    let unchanged = || {
        assert_eq!(read_files(&paths), original_bytes);
        assert_eq!(
            directories.each_ref().map(|path| directory_entries(path)),
            original_members
        );
        assert_no_attempt_or_result(&fixture, &run_dir, &ledger);
        assert_eq!(inspect_ledger(), unknown);
        assert_eq!(io.writes.as_slice(), std::slice::from_ref(first));
        assert_eq!(io.current[first], after);
    };
    let request = StageRequest {
        backend: &fixture.backend,
        run_dir: &run_dir,
        stage: "copy",
        mode: "reconcile",
        attempt: None,
    };
    Runner.preflight(&request).unwrap();

    let source = binding_path(&manifest, "source_environment");
    let original_source = fs::read(&source).unwrap();
    let mut changed = document::read(&source).unwrap();
    assert_eq!(changed, json!({"environment":{"APP_ENV":"test"}}));
    changed["environment"]["NEW_INPUT"] = json!("");
    fs::write(&source, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert_eq!(document::read(&source).unwrap(), changed);
    let error =
        stage::run_named(&fixture.backend, &run_dir, "copy", "reconcile", &Runner).unwrap_err();
    assert_eq!(error.to_string(), "source_environment与登记摘要不一致");
    unchanged();

    fs::write(&source, &original_source).unwrap();
    assert_eq!(fs::read(&source).unwrap(), original_source);
    Runner.preflight(&request).unwrap();
    unchanged();
}

fn binding_path(manifest: &Value, field: &str) -> PathBuf {
    PathBuf::from(manifest[field]["path"].as_str().unwrap())
}

fn original_paths(fixture: &Fixture, run_dir: &Path, manifest: &Value) -> Vec<PathBuf> {
    [
        "input.json",
        "plan.json",
        "session.json",
        "failure.json",
        "ledger/events.ndjson",
        "ledger/head.json",
    ]
    .map(|name| fixture.copy.join(name))
    .into_iter()
    .chain([
        run_dir.join("state.json"),
        run_dir.join("manifest.json"),
        run_dir.parent().unwrap().join("copy-manifest.json"),
    ])
    .chain(
        [
            "source_request",
            "source_export",
            "initialized",
            "target_environment",
        ]
        .map(|field| binding_path(manifest, field)),
    )
    .collect()
}

fn read_files(paths: &[PathBuf]) -> Vec<Vec<u8>> {
    paths.iter().map(|path| fs::read(path).unwrap()).collect()
}

fn directory_entries(path: &Path) -> BTreeSet<PathBuf> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

fn assert_no_attempt_or_result(fixture: &Fixture, run_dir: &Path, ledger: &Path) {
    let current = document::read(&run_dir.join("state.json")).unwrap();
    assert_eq!(current["attempts"], json!([]));
    assert!(!run_dir.join("run.lock").exists());
    assert!(!state::controller_path(run_dir, 1).exists());
    assert!(!state::result_path(run_dir, 1).exists());
    assert!(!fixture.copy.join("result.json").exists());
    assert_eq!(completion_count(ledger), 0);
}
