use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use super::{
    finish_cases::{
        completion_count, database_writes, four_database_declaration, run_published_copy,
    },
    *,
};
use crate::{
    clone_native_tests::{Fixture as OfflineFixture, snapshot},
    recovery::clone::native::{
        copy_transfer::{
            Runner,
            proofs::Proofs,
            record::{Facts, Files, Prepared},
        },
        digest,
        lifecycle::stage::{self, StageRequest, StageRunner},
    },
};

#[test]
fn orphaned_empty_copy_needs_reconciliation_before_hydration() {
    let mut fixture = Fixture::new();
    let mut declaration = four_database_declaration(1);
    declaration["format_version"] = json!(1);
    declaration["copy_id"] = json!("orphaned-copy");
    declaration["copy_stage"] = json!("source_to_seed");
    declaration["pending_target_actions"] = json!([]);
    let (run_dir, manifest) = fixture.register_copy_run().unwrap();
    let prepared = bind_copy_files(&fixture, &manifest, &declaration).unwrap();
    let plan = Plan::from_verified(&prepared.plan).unwrap();
    let mut io = FixtureIo::new(&plan);
    database_writes(&mut io, &plan.databases[..2]);
    io.database_fault = Some(DatabaseFault::Checkpoint);
    let error = run_published_copy(&fixture, &prepared, &plan, Mode::New, &mut io).unwrap_err();
    assert_eq!(error.to_string(), "测试：数据库观察后代次校验失败");
    let fixed_paths = ["input.json", "plan.json", "session.json", "failure.json"]
        .map(|name| fixture.copy.join(name));
    let original = fixed_paths.each_ref().map(|path| fs::read(path).unwrap());
    let failure = document::read(&fixed_paths[3]).unwrap();
    assert_eq!(failure["phase"], "transfer");
    assert_eq!(failure["status"], "needs_reconciliation");
    assert!(io.writes.is_empty());

    fixture.open_orphaned_reconciliation().unwrap();
    let ledger = fixture.copy.join("ledger");
    let journal = ledger.join("events.ndjson");
    let prefix = fs::read(&journal).unwrap();
    assert_orphaned_open(&prefix);
    assert_resume_preflight_rejects(&fixture, &run_dir);
    assert!(io.writes.is_empty());

    let preflight = |mode| {
        Runner.preflight(&StageRequest {
            backend: &fixture.backend,
            run_dir: &run_dir,
            stage: "copy",
            mode,
            attempt: None,
        })
    };
    preflight("reconcile").unwrap();
    let (reconciled, empty) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Reconcile, &mut io).unwrap();
    let pending = plan.all().map(|step| &step.id).collect::<BTreeSet<_>>();
    assert_eq!(pending.len(), 5);
    assert_eq!(reconciled["status"], "copy_reconciled");
    assert_eq!(reconciled["pending_steps"], json!(pending));
    assert_eq!(reconciled["remote_writes"], 0);
    assert_eq!(empty["steps"], json!({}));
    assert_eq!(empty["status"], "partial");
    assert!(io.writes.is_empty());
    assert!(!fixture.copy.join("result.json").exists());
    assert_eq!(completion_count(&ledger), 0);
    let reconciled_journal = fs::read(&journal).unwrap();
    let appended = reconciled_journal.strip_prefix(prefix.as_slice()).unwrap();
    let events = journal_events(appended);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "opened");
    assert_eq!(events[0]["mode"], "reconcile");
    assert_eq!(events[1]["type"], "closed");
    assert_eq!(events[1]["outcome"], "partial");

    preflight("resume").unwrap();
    let (result, verified) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Resume, &mut io).unwrap();
    let expected_writes = plan.databases[..2]
        .iter()
        .chain(&plan.objects)
        .map(|step| step.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(io.writes, expected_writes);
    assert_eq!(verified["status"], "ledger_evidence_complete");
    assert_eq!(
        verified["steps"].as_object().unwrap().len(),
        expected_writes.len()
    );
    for id in expected_writes {
        assert_eq!(verified["steps"][&id]["phase"], "confirmed");
        assert_eq!(verified["steps"][&id]["attempt"], 1);
    }
    let saved = document::read(&fixture.copy.join("result.json")).unwrap();
    assert_eq!(saved["status"], "data_steps_verified");
    assert_eq!(saved["written_steps"], 3);
    assert_eq!(saved["unchanged_databases"], 2);
    assert_eq!(saved["target_ready"], false);
    assert_eq!(saved["restore_success"], false);
    assert_eq!(result["status"], saved["status"]);
    assert_eq!(completion_count(&ledger), 1);
    assert!(fs::read(journal).unwrap().starts_with(&reconciled_journal));
    for (path, bytes) in fixed_paths.iter().zip(&original) {
        assert_eq!(&fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn empty_copy_reconciles_and_resumes_with_the_original_prepared_inputs() {
    let source = OfflineFixture::new();
    let initialized = json!({
        "inventory":{"observations":{"shared-control":{"kind":"database","tables":{},"preserved":{}}}}
    });
    let mut fixture = Fixture::in_root(&source.root, &initialized);
    let (run_dir, manifest, declaration, proofs) =
        register_original_inputs(&mut fixture, &source, &initialized);
    let source_stopped = &source.value["evidence"]["source_stopped"];
    let initial = &manifest["initialized"];
    let source_generation = json!({"stopped":true});
    let target_generation = json!({"initialized":true});
    let source_observation =
        json!({"shared-control":{},"shared":{},"dedicated-a":{},"dedicated-b":{}});
    let facts = || Facts {
        source_generation: &source_generation,
        target_generation: &target_generation,
        source_observation: &source_observation,
        declaration: &declaration,
    };
    let bind = || Files::bind(&fixture.backend, &manifest, &manifest["source_export"]).unwrap();
    let ledger = fixture.copy.join("ledger");
    // 复制账本尚未建立，只移除夹具预置的空会话，让正式准备写入原件。
    assert!(!ledger.exists());
    fs::remove_file(fixture.copy.join("session.json")).unwrap();
    let initialized_path = Path::new(initial["path"].as_str().unwrap());
    let (mut io, failed) = fail_before_first_intent(
        &fixture,
        &source.export,
        initialized_path,
        bind(),
        facts(),
        proofs,
    );
    let journal = ledger.join("events.ndjson");
    let result_file = fixture.copy.join("result.json");
    assert_resume_preflight_rejects(&fixture, &run_dir);
    // 正式 Runner 负责只读预检，受管账本与发布逻辑使用隔离的 I/O 替身。
    let continue_saved = |name: &str, mode, io: &mut FixtureIo| {
        Runner
            .preflight(&StageRequest {
                backend: &fixture.backend,
                run_dir: &run_dir,
                stage: "copy",
                mode: name,
                attempt: None,
            })
            .unwrap();
        let files = bind();
        let saved = files.continuation("source_to_seed").unwrap();
        let proofs = Proofs::open(
            &source.export,
            &saved.declaration["evidence"],
            source_stopped,
            initial,
            &initialized,
        )
        .unwrap();
        let hydrated = files.hydrate(&saved, facts()).unwrap();
        assert_eq!(proofs.descriptors, declaration["evidence"]);
        assert_eq!(hydrated.plan, failed.plan);
        assert_eq!(hydrated.record, failed.record);
        let plan = Plan::from_verified(&hydrated.plan).unwrap();
        let result = run_published_copy(&fixture, &hydrated, &plan, mode, io).unwrap();
        proofs.checkpoint().unwrap();
        assert_original_inputs_retained(&fixture.copy, &source.export, initialized_path, &failed);
        result
    };
    let (reconciled, empty) = continue_saved("reconcile", Mode::Reconcile, &mut io);
    assert!(io.writes.is_empty());
    assert!(!result_file.exists());
    assert_eq!(completion_count(&ledger), 0);
    let partial = fs::read(&journal).unwrap();
    assert_empty_reconcile(&reconciled, &empty, &failed, &partial);

    let (result, verified) = continue_saved("resume", Mode::Resume, &mut io);
    assert_eq!(io.writes, failed.planned);
    assert_resumed_copy(&result_file, &result, &verified, &manifest, &failed.planned);
    assert_eq!(completion_count(&ledger), 1);
    assert!(fs::read(journal).unwrap().starts_with(&partial));
}

struct EmptyCopyFailure {
    planned: Vec<String>,
    plan: Value,
    record: Value,
    input_paths: [PathBuf; 3],
    inputs: [Vec<u8>; 3],
    export: BTreeMap<PathBuf, Vec<u8>>,
    export_entries: BTreeSet<PathBuf>,
    initialized: Vec<u8>,
    receipt: Vec<u8>,
    journal: Vec<u8>,
}

fn fail_before_first_intent(
    fixture: &Fixture,
    export_root: &Path,
    initialized_path: &Path,
    files: Files<'_>,
    facts: Facts<'_>,
    proofs: Proofs,
) -> (FixtureIo, EmptyCopyFailure) {
    let prepared = files.prepare(facts).unwrap();
    drop(files);
    proofs.checkpoint().unwrap();
    let plan = Plan::from_verified(&prepared.plan).unwrap();
    assert_eq!(plan.databases.len(), 2);
    assert_eq!(plan.objects.len(), 1);
    let planned = plan.all().map(|step| step.id.clone()).collect::<Vec<_>>();
    let input_paths =
        ["input.json", "plan.json", "session.json"].map(|name| fixture.copy.join(name));
    let inputs = input_paths.each_ref().map(|path| fs::read(path).unwrap());
    let export = snapshot(export_root);
    let export_entries = directory_entries(export_root);
    let initialized = fs::read(initialized_path).unwrap();
    let mut io = FixtureIo::new(&plan);
    database_writes(&mut io, &plan.databases);
    io.database_fault = Some(DatabaseFault::Checkpoint);
    let error = run_published_copy(fixture, &prepared, &plan, Mode::New, &mut io).unwrap_err();
    assert_eq!(error.to_string(), "测试：数据库观察后代次校验失败");
    assert!(matches!(
        io.database_events.as_slice(),
        [DatabaseEvent::Observe(_)]
    ));
    assert!(io.writes.is_empty());
    let original_plan = prepared.plan.clone();
    let original_record = prepared.record.clone();
    drop((prepared, proofs));

    let failure_path = fixture.copy.join("failure.json");
    let failure = document::read(&failure_path).unwrap();
    let receipt = fs::read(&failure_path).unwrap();
    assert_eq!(failure["phase"], "transfer");
    assert_eq!(failure["status"], "needs_reconciliation");
    assert!(!fixture.copy.join("result.json").exists());
    let journal = fs::read(fixture.copy.join("ledger/events.ndjson")).unwrap();
    let events = journal_events(&journal);
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["type"], "created");
    assert_eq!(events[1]["type"], "opened");
    assert_eq!(events[1]["mode"], "apply");
    assert_eq!(events[2]["type"], "closed");
    assert_eq!(events[2]["outcome"], "failed");
    let failed = EmptyCopyFailure {
        planned,
        plan: original_plan,
        record: original_record,
        input_paths,
        inputs,
        export,
        export_entries,
        initialized,
        receipt,
        journal,
    };
    (io, failed)
}

fn register_original_inputs(
    fixture: &mut Fixture,
    source: &OfflineFixture,
    initialized: &Value,
) -> (PathBuf, Value, Value, Proofs) {
    let mut declaration = source.value.clone();
    let source_stopped = &source.value["evidence"]["source_stopped"];
    let (run_dir, manifest) = fixture
        .register_copy_run_with_export(
            &source.export,
            json!({
                "artifact_root":source.export,"evidence":{"generation-after":source_stopped},
                "databases":declaration["databases"],"objects":declaration["objects"],
            }),
        )
        .unwrap();
    let initial = &manifest["initialized"];
    let stopped = json!({
        "status":"fresh_target_reverified","initialized":initial,
        "inventory":initialized["inventory"],"remote_writes":0,
        "target_ready":false,"clone_verified":false,"restore_qualified":false,
    });
    let proofs = Proofs::publish(
        &fixture.backend,
        &source.export,
        source_stopped,
        initial,
        initialized,
        &stopped,
    )
    .unwrap();
    declaration["copy_id"] = manifest["id"].clone();
    declaration["evidence"] = proofs.descriptors.clone();
    (run_dir, manifest, declaration, proofs)
}

fn assert_empty_reconcile(
    result: &Value,
    ledger: &Value,
    failed: &EmptyCopyFailure,
    partial: &[u8],
) {
    assert_eq!(result["status"], "copy_reconciled");
    assert_eq!(
        result["pending_steps"],
        json!(failed.planned.iter().collect::<BTreeSet<_>>())
    );
    assert_eq!(result["remote_writes"], 0);
    assert_eq!(ledger["steps"], json!({}));
    assert_eq!(ledger["status"], "partial");
    let events = journal_events(partial.strip_prefix(failed.journal.as_slice()).unwrap());
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "opened");
    assert_eq!(events[0]["mode"], "reconcile");
    assert_eq!(events[1]["type"], "closed");
    assert_eq!(events[1]["outcome"], "partial");
}

fn assert_original_inputs_retained(
    copy: &Path,
    export_root: &Path,
    initialized_path: &Path,
    failed: &EmptyCopyFailure,
) {
    assert_eq!(
        failed
            .input_paths
            .each_ref()
            .map(|path| fs::read(path).unwrap()),
        failed.inputs
    );
    assert_eq!(snapshot(export_root), failed.export);
    assert_eq!(directory_entries(export_root), failed.export_entries);
    assert_eq!(fs::read(initialized_path).unwrap(), failed.initialized);
    assert_eq!(fs::read(copy.join("failure.json")).unwrap(), failed.receipt);
}

fn assert_resumed_copy(
    result_file: &Path,
    result: &Value,
    verified: &Value,
    manifest: &Value,
    planned: &[String],
) {
    assert_eq!(verified["status"], "ledger_evidence_complete");
    assert_eq!(verified["steps"].as_object().unwrap().len(), 3);
    for id in planned {
        assert_eq!(verified["steps"][id]["phase"], "confirmed");
        assert_eq!(verified["steps"][id]["attempt"], 1);
    }
    let saved = document::read(result_file).unwrap();
    assert_eq!(result["status"], "data_steps_verified");
    assert_eq!(saved["status"], result["status"]);
    assert_eq!(saved["written_steps"], 3);
    assert_eq!(saved["unchanged_databases"], 0);
    assert_eq!(
        saved["source_export_sha256"],
        manifest["source_export"]["sha256"]
    );
    assert_eq!(
        saved["fresh_target_sha256"],
        manifest["initialized"]["sha256"]
    );
}

pub(super) fn bind_copy_files(
    fixture: &Fixture,
    manifest: &Value,
    declaration: &Value,
) -> Result<Prepared> {
    let input = fixture.copy.join("input.json");
    document::write_new(&input, declaration)?;
    let binding = digest::file(&input)?;
    let mut plan = declaration.clone();
    plan["kind"] = json!("devex-clone-offline-plan");
    plan["status"] = json!("offline_verified");
    plan["execution_authorized"] = json!(false);
    plan["target_ready"] = json!(false);
    plan["input_sha256"] = json!(digest::plan_hash(declaration)?);
    plan["plan_sha256"] = json!(digest::plan_hash(&plan)?);
    document::write_new(
        &fixture.copy.join("plan.json"),
        &json!({
            "format_version":1,"input_path":input,
            "input_file":{"bytes":binding.bytes,"sha256":binding.sha256},"plan":plan,
        }),
    )?;
    let facts = Facts {
        source_generation: &json!({"source":true}),
        target_generation: &json!({"target":true}),
        source_observation: &json!({"shared-control":{},"shared":{},"dedicated-a":{},"dedicated-b":{}}),
        declaration,
    };
    let files = Files::bind(&fixture.backend, manifest, &manifest["source_export"])?;
    let record = files.value(&facts, &plan)?;
    let session = fixture.copy.join("session.json");
    // 此时还没有复制控制器，替换夹具初建的空会话。
    fs::remove_file(&session)?;
    document::write_new(&session, &record)?;
    Prepared::from_test_session(&fixture.copy, plan)
}

fn assert_orphaned_open(journal: &[u8]) {
    let events = journal_events(journal);
    assert_eq!(events.len(), 4);
    assert_eq!(events[0]["type"], "created");
    assert_eq!(events[1]["type"], "opened");
    assert_eq!(events[1]["mode"], "apply");
    assert_eq!(events[2]["type"], "closed");
    assert_eq!(events[2]["outcome"], "failed");
    assert_eq!(events[3]["type"], "opened");
    assert_eq!(events[3]["mode"], "reconcile");
}

fn assert_resume_preflight_rejects(fixture: &Fixture, run_dir: &Path) {
    let paths = [
        fixture.copy.join("ledger/events.ndjson"),
        fixture.copy.join("ledger/head.json"),
        fixture.copy.join("failure.json"),
        fixture.copy.join("input.json"),
        fixture.copy.join("plan.json"),
        fixture.copy.join("session.json"),
        run_dir.join("state.json"),
    ];
    let original = paths.each_ref().map(|path| fs::read(path).unwrap());
    let directories = [run_dir.to_owned(), fixture.copy.clone()];
    let original_members = directories.each_ref().map(|dir| directory_entries(dir));
    let error = stage::run_named(&fixture.backend, run_dir, "copy", "resume", &Runner).unwrap_err();
    assert_eq!(
        error.to_string(),
        "原复制会话未正常关闭或有未知写入，必须先显式核对"
    );
    assert_eq!(
        paths.each_ref().map(|path| fs::read(path).unwrap()),
        original
    );
    assert_eq!(
        directories.each_ref().map(|dir| directory_entries(dir)),
        original_members
    );
    assert_eq!(
        document::read(&run_dir.join("state.json")).unwrap()["attempts"],
        json!([])
    );
    assert!(!run_dir.join("run.lock").exists());
    assert!(!fixture.copy.join("ledger/clone.lock").exists());
    assert!(
        !fixture
            .backend
            .join(".local-tests/target/initialize.lock")
            .exists()
    );
}

fn journal_events(bytes: &[u8]) -> Vec<Value> {
    serde_json::Deserializer::from_slice(bytes)
        .into_iter::<Value>()
        .map(|frame| frame.unwrap()["event"].clone())
        .collect()
}

fn directory_entries(path: &Path) -> BTreeSet<PathBuf> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}
