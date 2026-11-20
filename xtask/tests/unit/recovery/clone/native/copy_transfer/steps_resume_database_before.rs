use std::{fs, path::Path};

use super::{
    finish_cases::{
        completion_count, database_writes, four_database_declaration, run_published_copy,
    },
    resume_orphan::bind_copy_files,
    *,
};
use crate::recovery::clone::native::copy_ledger::{check_continuation, inspect};

#[test]
fn reconciled_database_before_image_gets_one_explicit_resume_attempt() {
    let mut fixture = Fixture::new();
    let (_, manifest) = fixture.register_copy_run().unwrap();
    let mut declaration = four_database_declaration(1);
    declaration["format_version"] = json!(1);
    declaration["copy_id"] = manifest["id"].clone();
    declaration["copy_stage"] = manifest["copy_stage"].clone();
    declaration["pending_target_actions"] = json!([]);
    let prepared = bind_copy_files(&fixture, &manifest, &declaration).unwrap();
    let plan = Plan::from_verified(&prepared.plan).unwrap();
    let first = &plan.databases[0];
    let mut io = FixtureIo::new(&plan);
    database_writes(&mut io, &plan.databases[..2]);
    let first_basis = io.bases[&first.id].clone();
    let inputs = ["input.json", "plan.json", "session.json"].map(|name| fixture.copy.join(name));
    let original_inputs = inputs.each_ref().map(|path| fs::read(path).unwrap());

    io.stop_at = Some(first.id.clone());
    let error = run_published_copy(&fixture, &prepared, &plan, Mode::New, &mut io).unwrap_err();
    assert_eq!(error.to_string(), "复制进程结束，资源写入结果未知");
    assert_eq!(io.writes.as_slice(), std::slice::from_ref(&first.id));
    assert_eq!(io.current[&first.id], first_basis["before"]);
    io.stop_at = None;

    let ledger = fixture.copy.join("ledger");
    let journal = ledger.join("events.ndjson");
    let head = ledger.join("head.json");
    let failure = fixture.copy.join("failure.json");
    let fixed = [&failure, &journal, &head];
    let original = fixed.map(|path| fs::read(path).unwrap());
    assert_initial_failure(&failure);
    let plan_sha = prepared.record["plan_sha256"].as_str().unwrap();
    let generation = prepared.record["generation_sha256"].as_str().unwrap();
    let failed = inspect(&fixture.backend, &ledger, plan_sha, generation).unwrap();
    assert_eq!(failed["status"], "needs_reconciliation");
    assert_eq!(failed["steps"].as_object().unwrap().len(), 1);
    assert_step(
        &failed["steps"][&first.id],
        first,
        &first_basis,
        "unknown",
        1,
    );

    let check = |mode| check_continuation(&fixture.backend, &ledger, plan_sha, generation, mode);
    let error = check(Mode::Resume).unwrap_err();
    assert_eq!(
        error.to_string(),
        "原复制会话未正常关闭或有未知写入，必须先显式核对"
    );
    assert_eq!(fixed.map(|path| fs::read(path).unwrap()), original);
    assert_eq!(io.writes.as_slice(), std::slice::from_ref(&first.id));

    check(Mode::Reconcile).unwrap();
    io.database_events.clear();
    let (reconciled, reconciled_ledger) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Reconcile, &mut io).unwrap();
    assert_reconciliation(&plan, &io, &reconciled);
    assert_eq!(reconciled_ledger["status"], "needs_reconciliation");
    assert_step(
        &reconciled_ledger["steps"][&first.id],
        first,
        &first_basis,
        "reconciled_before",
        1,
    );
    assert!(!fixture.copy.join("result.json").exists());
    assert_eq!(completion_count(&ledger), 0);
    assert_eq!(fs::read(&failure).unwrap(), original[0]);
    let reconciled_prefix = fs::read(&journal).unwrap();
    assert_reconciled_events(&original[1], &reconciled_prefix, first, &first_basis);

    check(Mode::Resume).unwrap();
    io.database_events.clear();
    io.object_batches.clear();
    let (result, completed) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Resume, &mut io).unwrap();
    assert_completed_resume(&plan, &io, &completed);
    assert_result(&fixture.copy, &result, &plan, &io);
    assert_eq!(completion_count(&ledger), 1);
    assert_eq!(
        inspect(&fixture.backend, &ledger, plan_sha, generation).unwrap(),
        completed
    );
    assert_resumed_events(
        &reconciled_prefix,
        &fs::read(&journal).unwrap(),
        first,
        &first_basis,
    );
    assert_eq!(fs::read(failure).unwrap(), original[0]);
    assert_eq!(
        inputs.each_ref().map(|path| fs::read(path).unwrap()),
        original_inputs
    );
}

fn assert_initial_failure(path: &Path) {
    let saved = document::read(path).unwrap();
    assert_eq!(saved["phase"], "transfer");
    assert_eq!(saved["status"], "needs_reconciliation");
    assert_eq!(saved["automatic_retry"], false);
}

fn assert_step(saved: &Value, step: &Step<'_>, basis: &Value, phase: &str, attempt: u64) {
    assert_eq!(saved["phase"], phase);
    assert_eq!(saved["attempt"], attempt);
    assert_eq!(saved["before"], basis["before"]);
    assert_eq!(saved["after"], basis["after"]);
    assert_eq!(saved["artifact_sha256"], step.artifact);
}

fn assert_reconciliation(plan: &Plan<'_>, io: &FixtureIo, result: &Value) {
    let first = &plan.databases[0].id;
    assert_eq!(result["status"], "copy_reconciled");
    assert_eq!(result["steps"], json!({(first):"reconciled_before"}));
    let pending = plan
        .all()
        .skip(1)
        .map(|step| &step.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(pending.len(), 4);
    assert_eq!(result["pending_steps"], json!(pending));
    assert_eq!(result["remote_writes"], 0);
    assert_eq!(
        io.database_events,
        [DatabaseEvent::Observe(io.database_ids.clone())]
    );
    assert_eq!(io.object_batches, [vec![plan.objects[0].id.clone()]]);
    assert_eq!(io.writes.as_slice(), std::slice::from_ref(first));
    assert_eq!(io.complete_checks, 0);
}

fn assert_completed_resume(plan: &Plan<'_>, io: &FixtureIo, completed: &Value) {
    let [first, second] = [&plan.databases[0], &plan.databases[1]];
    let object = &plan.objects[0];
    assert_eq!(completed["status"], "ledger_evidence_complete");
    assert_eq!(completed["steps"].as_object().unwrap().len(), 3);
    for (step, attempt) in [(first, 2), (second, 1), (object, 1)] {
        let basis = &io.bases[&step.id];
        assert_step(
            &completed["steps"][&step.id],
            step,
            basis,
            "confirmed",
            attempt,
        );
        assert_eq!(io.current[&step.id], basis["after"]);
    }
    assert_eq!(
        io.writes,
        [
            first.id.clone(),
            first.id.clone(),
            second.id.clone(),
            object.id.clone()
        ]
    );
    let observe_databases = || DatabaseEvent::Observe(io.database_ids.clone());
    assert_eq!(
        io.database_events,
        [
            observe_databases(),
            DatabaseEvent::Write(first.id.clone()),
            observe_databases(),
            observe_databases(),
            DatabaseEvent::Write(second.id.clone()),
            observe_databases(),
            observe_databases(),
            observe_databases(),
            DatabaseEvent::WriteObject(object.id.clone()),
            observe_databases(),
            DatabaseEvent::VerifyComplete,
        ]
    );
    assert_eq!(
        io.object_batches,
        [
            vec![object.id.clone()],
            vec![object.id.clone()],
            vec![object.id.clone()]
        ]
    );
    assert_eq!(io.complete_checks, 1);
}

fn assert_result(copy: &Path, result: &Value, plan: &Plan<'_>, io: &FixtureIo) {
    let saved = document::read(&copy.join("result.json")).unwrap();
    assert_eq!(result["status"], saved["status"]);
    assert_eq!(saved["status"], "data_steps_verified");
    assert_eq!(saved["written_steps"], 3);
    assert_eq!(saved["unchanged_databases"], 2);
    assert_eq!(saved["target_ready"], false);
    assert_eq!(saved["restore_success"], false);
    for step in &plan.databases[2..] {
        let before = &io.bases[&step.id]["before"];
        assert_eq!(&io.current[&step.id], before);
        assert_eq!(&saved["unchanged_database_images"][&step.id], before);
    }
}

fn assert_reconciled_events(original: &[u8], reconciled: &[u8], first: &Step<'_>, basis: &Value) {
    assert_eq!(
        step_events(original, &first.id),
        [
            json!({"type":"intent","step_id":first.id,"before":basis["before"],
                "after":basis["after"],"artifact_sha256":first.artifact})
        ]
    );
    let appended = reconciled.strip_prefix(original).unwrap();
    let events = serde_json::Deserializer::from_slice(appended)
        .into_iter::<Value>()
        .map(|frame| frame.unwrap()["event"].clone())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["type"], "opened");
    assert_eq!(events[0]["mode"], "reconcile");
    assert_eq!(
        events[1],
        json!({"type":"reconciled","step_id":first.id,"observed":basis["before"]})
    );
    assert_eq!(events[2]["type"], "closed");
    assert_eq!(events[2]["outcome"], "partial");
}

fn assert_resumed_events(reconciled: &[u8], final_journal: &[u8], first: &Step<'_>, basis: &Value) {
    assert_eq!(
        step_events(final_journal.strip_prefix(reconciled).unwrap(), &first.id),
        [
            json!({"type":"resumed","step_id":first.id,"observed":basis["before"]}),
            json!({"type":"confirmed","step_id":first.id,"observed":basis["after"]}),
        ]
    );
}

fn step_events(journal: &[u8], step: &str) -> Vec<Value> {
    serde_json::Deserializer::from_slice(journal)
        .into_iter::<Value>()
        .map(|frame| frame.unwrap()["event"].clone())
        .filter(|event| event["step_id"] == step)
        .collect()
}
