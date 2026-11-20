use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
};

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
    resources::common,
};

#[test]
fn reconcile_preserves_residual_target_and_ledger_locks_after_unknown_write() {
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
    let target = fixture.backend.join(".local-tests/target");
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
    let fixed = [
        fixture.copy.join("input.json"),
        fixture.copy.join("plan.json"),
        fixture.copy.join("session.json"),
        fixture.copy.join("failure.json"),
        ledger.join("events.ndjson"),
        ledger.join("head.json"),
    ];
    let original = fixed.each_ref().map(|path| fs::read(path).unwrap());
    let copy_members = directory_entries(&fixture.copy);
    let run_members = directory_entries(&run_dir);
    let target_members = directory_entries(&target);
    let ledger_members = directory_entries(&ledger);
    let state_path = run_dir.join("state.json");
    let original_state = fs::read(&state_path).unwrap();

    with_residual_lock(&ledger.join("clone.lock"), || {
        let error =
            stage::run_named(&fixture.backend, &run_dir, "copy", "reconcile", &Runner).unwrap_err();
        assert_eq!(
            error.to_string(),
            "原复制账本已持锁或模式无效，必须先核对登记的原控制器"
        );
        assert_eq!(fs::read(&state_path).unwrap(), original_state);
        assert_eq!(directory_entries(&run_dir), run_members);
        assert_eq!(directory_entries(&fixture.copy), copy_members);
        assert_eq!(
            fixed.each_ref().map(|path| fs::read(path).unwrap()),
            original
        );
    });

    with_residual_lock(&target.join("initialize.lock"), || {
        assert_target_lock_failure(&fixture, &run_dir, &run_members, &copy_members);
        assert_eq!(
            fixed.each_ref().map(|path| fs::read(path).unwrap()),
            original
        );
    });

    assert_eq!(directory_entries(&target), target_members);
    assert_eq!(directory_entries(&ledger), ledger_members);
    assert_eq!(inspect_ledger(), unknown);
    assert_eq!(io.writes.as_slice(), std::slice::from_ref(first));
    assert_eq!(io.current[first], after);
    assert!(!fixture.copy.join("result.json").exists());
    assert_eq!(completion_count(&ledger), 0);
}

#[derive(Default)]
struct RecordingRunner {
    execute_error: RefCell<Option<String>>,
}

impl StageRunner for RecordingRunner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        Runner.preflight(request)
    }

    fn guarded_preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        Runner.guarded_preflight(request)
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        let result = Runner.execute(request);
        *self.execute_error.borrow_mut() = result.as_ref().err().map(ToString::to_string);
        result
    }
}

fn assert_target_lock_failure(
    fixture: &Fixture,
    run_dir: &Path,
    run_members: &BTreeSet<PathBuf>,
    copy_members: &BTreeSet<PathBuf>,
) {
    let runner = RecordingRunner::default();
    let error =
        stage::run_named(&fixture.backend, run_dir, "copy", "reconcile", &runner).unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("阶段 copy 未完成：StageError")
    );
    assert_eq!(
        runner.execute_error.into_inner().as_deref(),
        Some("目标初始化锁已存在，不能并发执行或自动抢锁")
    );

    let saved = document::read(&run_dir.join("state.json")).unwrap();
    let attempts = saved["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["number"], 1);
    assert_eq!(attempts[0]["stage"], "copy");
    assert_eq!(attempts[0]["mode"], "reconcile");
    assert_eq!(attempts[0]["status"], "failed");
    assert_eq!(attempts[0]["error_type"], "StageError");
    assert!(attempts[0]["result"].is_null());
    let controller = state::controller_path(run_dir, 1);
    let failure_path = run_dir.join("failure-0001.json");
    let failure = document::read(&failure_path).unwrap();
    assert_eq!(failure["controller"], common::binding(&controller).unwrap());
    assert_eq!(
        directory_entries(run_dir),
        run_members
            .iter()
            .cloned()
            .chain([controller, failure_path, run_dir.join("run-control.guard")])
            .collect::<BTreeSet<_>>()
    );
    assert!(directory_entries(&run_dir.join("results")).is_empty());
    assert!(!run_dir.join("run.lock").exists());

    let current = directory_entries(&fixture.copy);
    assert!(copy_members.is_subset(&current));
    let added = current
        .difference(copy_members)
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    assert_eq!(added.len(), 2);
    let owner_path = find_receipt(&added, "copy-owner-");
    let owner = document::read(owner_path).unwrap();
    assert_eq!(owner["kind"], "devex-copy-owner");
    assert_eq!(
        owner["session"],
        common::binding(&fixture.copy.join("session.json")).unwrap()
    );
    let continuation = document::read(find_receipt(&added, "continuation-")).unwrap();
    assert_eq!(continuation["mode"], "reconcile");
    assert_eq!(continuation["status"], "needs_reconciliation");
    assert_eq!(continuation["automatic_retry"], false);
    assert_eq!(continuation["automatic_resource_cleanup"], false);
}

fn find_receipt<'a>(entries: &[&'a Path], prefix: &str) -> &'a Path {
    entries
        .iter()
        .copied()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".json"))
        })
        .unwrap()
}

fn with_residual_lock(lock: &Path, reconcile: impl FnOnce()) {
    let identity = || {
        let metadata = fs::symlink_metadata(lock).unwrap();
        assert!(metadata.is_dir());
        common::device_inode_for(lock, &metadata).unwrap()
    };
    fs::create_dir(lock).unwrap();
    let original = identity();
    reconcile();
    assert_eq!(identity(), original);
    assert!(directory_entries(lock).is_empty());
    fs::remove_dir(lock).unwrap();
}

fn directory_entries(path: &Path) -> BTreeSet<PathBuf> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}
