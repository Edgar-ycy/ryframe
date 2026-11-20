use super::*;
use crate::recovery::clone::native::copy_ledger::inspect;

#[test]
fn changed_confirmed_database_postimage_blocks_resume_before_new_writes() {
    let fixture = Fixture::new();
    let mut declaration = four_database_declaration(1);
    declaration["pending_target_actions"] = json!([]);
    let prepared = Prepared::from_test_session(&fixture.copy, declaration).unwrap();
    let plan = Plan::from_verified(&prepared.plan).unwrap();
    let mut io = FixtureIo::new(&plan);
    database_writes(&mut io, &plan.databases[..2]);
    let first = &plan.databases[0].id;
    let before = io.bases[first]["before"].clone();
    let after = io.bases[first]["after"].clone();
    io.stop_at = Some(first.clone());
    io.failed_database_image = Some(after.clone());
    let error = run_published_copy(&fixture, &prepared, &plan, Mode::New, &mut io).unwrap_err();
    assert_eq!(error.to_string(), "复制进程结束，资源写入结果未知");
    assert_eq!(io.writes.as_slice(), std::slice::from_ref(first));
    assert_eq!(io.current[first], after);
    io.stop_at = None;

    let failure_path = fixture.copy.join("failure.json");
    let original_failure = fs::read(&failure_path).unwrap();
    let session_path = fixture.copy.join("session.json");
    let original_session = fs::read(&session_path).unwrap();
    let ledger = fixture.copy.join("ledger");
    let plan_sha = prepared.record["plan_sha256"].as_str().unwrap();
    let generation = prepared.record["generation_sha256"].as_str().unwrap();
    check_continuation(
        &fixture.backend,
        &ledger,
        plan_sha,
        generation,
        Mode::Reconcile,
    )
    .unwrap();
    let (reconciled, confirmed) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Reconcile, &mut io).unwrap();
    assert_eq!(reconciled["status"], "copy_reconciled");
    assert_eq!(reconciled["remote_writes"], 0);
    assert_eq!(confirmed["status"], "partial");
    assert_eq!(confirmed["steps"][first]["phase"], "confirmed");
    assert_eq!(confirmed["steps"][first]["attempt"], 1);
    let reconcile_path = Path::new(reconciled["continuation_receipt"].as_str().unwrap());
    let reconcile_receipt = document::read(reconcile_path).unwrap();
    let pending = plan
        .all()
        .filter(|step| &step.id != first)
        .map(|step| &step.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(reconcile_receipt["result"]["pending_steps"], json!(pending));
    assert_eq!(reconcile_receipt["ledger"], confirmed);
    assert_eq!(fs::read(&failure_path).unwrap(), original_failure);

    let journal_prefix = fs::read(ledger.join("events.ndjson")).unwrap();
    assert_eq!(before["resource_sha256"], after["resource_sha256"]);
    assert_ne!(before["tables_sha256"], after["tables_sha256"]);
    assert_eq!(io.current.insert(first.clone(), before), Some(after));
    io.database_events.clear();
    io.object_batches.clear();
    check_continuation(
        &fixture.backend,
        &ledger,
        plan_sha,
        generation,
        Mode::Resume,
    )
    .unwrap();
    let error = run_published_copy(&fixture, &prepared, &plan, Mode::Resume, &mut io).unwrap_err();
    assert_eq!(
        error.to_string(),
        "完整后像未确认或当前观察不同，必须停止并显式核对"
    );
    assert_eq!(
        io.database_events,
        [DatabaseEvent::Observe(io.database_ids.clone())]
    );
    assert!(io.object_batches.is_empty());
    assert_eq!(io.writes.as_slice(), std::slice::from_ref(first));
    assert_eq!(io.complete_checks, 0);
    assert!(!fixture.copy.join("result.json").exists());
    assert_eq!(completion_count(&ledger), 0);
    let failed = inspect(&fixture.backend, &ledger, plan_sha, generation).unwrap();
    assert_eq!(failed["status"], "needs_reconciliation");
    assert_eq!(failed["steps"], confirmed["steps"]);
    assert_eq!(fs::read(&session_path).unwrap(), original_session);
    assert_eq!(fs::read(&failure_path).unwrap(), original_failure);
    assert_recorded_resume_failure(&fixture.copy, &journal_prefix, reconcile_path);
}

fn assert_recorded_resume_failure(copy: &Path, journal_prefix: &[u8], reconcile_path: &Path) {
    let journal = fs::read(copy.join("ledger/events.ndjson")).unwrap();
    let appended = journal.strip_prefix(journal_prefix).unwrap();
    let events: Vec<Value> = serde_json::Deserializer::from_slice(appended)
        .into_iter()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["event"]["type"], "opened");
    assert_eq!(events[0]["event"]["mode"], "resume");
    assert_eq!(events[1]["event"]["type"], "closed");
    assert_eq!(events[1]["event"]["outcome"], "failed");

    let receipts = fs::read_dir(copy)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path != reconcile_path
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("continuation-") && name.ends_with(".json")
                    })
        })
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 1);
    let failure = document::read(&receipts[0]).unwrap();
    assert_eq!(failure["mode"], "resume");
    assert_eq!(failure["status"], "needs_reconciliation");
    assert_eq!(failure["automatic_retry"], false);
}
