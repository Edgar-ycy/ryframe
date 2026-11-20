use std::{fs, path::Path};

use super::{
    finish_cases::{
        completion_count, database_writes, four_database_declaration, run_published_copy,
    },
    *,
};
use crate::recovery::clone::native::{
    copy_transfer::{
        Runner,
        record::{Facts, Files, Prepared},
    },
    digest,
    lifecycle::stage::{self, StageRequest, StageRunner},
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
