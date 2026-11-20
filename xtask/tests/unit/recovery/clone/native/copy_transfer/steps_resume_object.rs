use std::{fs, path::Path};

use super::{
    finish_cases::{
        completion_count, database_writes, four_database_declaration, run_published_copy,
    },
    *,
};
use crate::recovery::clone::native::{
    copy_ledger::{check_continuation, inspect},
    copy_transfer::record::Prepared,
};

#[test]
fn lost_object_put_is_confirmed_without_repeating_object_or_database_writes() {
    let fixture = Fixture::new();
    let mut declaration = four_database_declaration(1);
    declaration["pending_target_actions"] = json!([]);
    let prepared = Prepared::from_test_session(&fixture.copy, declaration).unwrap();
    let plan = Plan::from_verified(&prepared.plan).unwrap();
    let mut io = FixtureIo::new(&plan);
    database_writes(&mut io, &plan.databases[..2]);
    let [first, second] = [&plan.databases[0].id, &plan.databases[1].id];
    let object = &plan.objects[0].id;
    let written = [first.clone(), second.clone(), object.clone()];
    let after = io.bases[object]["after"].clone();
    assert_object_postimage(&io.bases[object]["before"], &after);
    let inputs = [
        fixture.copy.join("session.json"),
        PathBuf::from(prepared.record["source_export"]["path"].as_str().unwrap()),
        PathBuf::from(prepared.record["initialized"]["path"].as_str().unwrap()),
    ];
    let original_inputs = inputs.each_ref().map(|path| fs::read(path).unwrap());
    io.stop_at = Some(object.clone());
    io.committed_write_image = Some(after.clone());
    let error = run_published_copy(&fixture, &prepared, &plan, Mode::New, &mut io).unwrap_err();
    assert_eq!(error.to_string(), "复制进程结束，资源写入结果未知");
    assert_eq!(io.writes, written);
    assert_eq!(io.current[object], after);
    assert_eq!(io.object_batches, [vec![object.clone()]]);
    io.stop_at = None;

    let ledger = fixture.copy.join("ledger");
    let journal = ledger.join("events.ndjson");
    let failure = fixture.copy.join("failure.json");
    let before = [&failure, &journal, &ledger.join("head.json")];
    let original = before.map(|path| fs::read(path).unwrap());
    let saved_failure = document::read(&failure).unwrap();
    assert_eq!(saved_failure["phase"], "transfer");
    assert_eq!(saved_failure["status"], "needs_reconciliation");
    assert_eq!(saved_failure["automatic_retry"], false);
    let plan_sha = prepared.record["plan_sha256"].as_str().unwrap();
    let generation = prepared.record["generation_sha256"].as_str().unwrap();
    let failed = inspect(&fixture.backend, &ledger, plan_sha, generation).unwrap();
    assert_eq!(failed["status"], "needs_reconciliation");
    assert_eq!(failed["steps"][object]["after"], after);
    assert_step_states(&failed["steps"], &written, "unknown");
    let check = |mode| check_continuation(&fixture.backend, &ledger, plan_sha, generation, mode);
    let error = check(Mode::Resume).unwrap_err();
    assert_eq!(
        error.to_string(),
        "原复制会话未正常关闭或有未知写入，必须先显式核对"
    );
    assert_eq!(io.writes, written);
    assert_eq!(before.map(|path| fs::read(path).unwrap()), original);
    assert_eq!(
        inputs.each_ref().map(|path| fs::read(path).unwrap()),
        original_inputs
    );

    check(Mode::Reconcile).unwrap();
    io.database_events.clear();
    io.object_batches.clear();
    let (reconciled, reconciled_ledger) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Reconcile, &mut io).unwrap();
    assert_reconciled(&plan, &io, &reconciled, &written);
    assert_eq!(reconciled_ledger["status"], "partial");
    assert_step_states(&reconciled_ledger["steps"], &written, "confirmed");
    assert!(!fixture.copy.join("result.json").exists());
    assert_eq!(completion_count(&ledger), 0);
    assert_eq!(fs::read(&failure).unwrap(), original[0]);
    let reconciled_prefix = fs::read(&journal).unwrap();
    assert_object_reconciliation(&original[1], &reconciled_prefix, object, &after);

    check(Mode::Resume).unwrap();
    io.database_events.clear();
    io.object_batches.clear();
    let (_, completed) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Resume, &mut io).unwrap();
    assert_eq!(io.writes, written);
    assert_resume_observations(&io, object);
    assert_eq!(completed["status"], "ledger_evidence_complete");
    assert_step_states(&completed["steps"], &written, "confirmed");
    assert_final_result(&fixture.copy, &ledger);
    assert_eq!(fs::read(&failure).unwrap(), original[0]);
    assert_eq!(
        inputs.each_ref().map(|path| fs::read(path).unwrap()),
        original_inputs
    );
    assert!(fs::read(journal).unwrap().starts_with(&reconciled_prefix));
}

fn assert_object_postimage(before: &Value, after: &Value) {
    assert_eq!(before["exists"], false);
    assert_eq!(
        after,
        &json!({
            "kind":"object","resource_sha256":before["resource_sha256"],"exists":true,
            "bytes":7,"sha256":ARTIFACT,"metadata_sha256":"d".repeat(64),
        })
    );
}

fn assert_step_states(steps: &Value, written: &[String; 3], object_phase: &str) {
    assert_eq!(steps.as_object().unwrap().len(), written.len());
    assert_eq!(steps[&written[0]]["phase"], "confirmed");
    assert_eq!(steps[&written[1]]["phase"], "confirmed");
    assert_eq!(steps[&written[2]]["phase"], object_phase);
    for id in written {
        assert_eq!(steps[id]["attempt"], 1);
    }
}

fn assert_reconciled(plan: &Plan<'_>, io: &FixtureIo, result: &Value, written: &[String; 3]) {
    assert_eq!(result["status"], "copy_reconciled");
    assert_eq!(result["remote_writes"], 0);
    assert_eq!(result["steps"].as_object().unwrap().len(), written.len());
    for id in written {
        assert_eq!(result["steps"][id], "confirmed");
    }
    let pending = plan.databases[2..]
        .iter()
        .map(|step| &step.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(result["pending_steps"], json!(pending));
    assert_eq!(
        io.database_events,
        [DatabaseEvent::Observe(io.database_ids.clone())]
    );
    assert_eq!(io.object_batches, [vec![written[2].clone()]]);
    assert_eq!(io.writes.as_slice(), written.as_slice());
    assert_eq!(io.complete_checks, 0);
}

fn assert_object_reconciliation(original: &[u8], reconciled: &[u8], object: &str, after: &Value) {
    let appended = reconciled.strip_prefix(original).unwrap();
    let events = serde_json::Deserializer::from_slice(appended)
        .into_iter::<Value>()
        .map(|frame| frame.unwrap()["event"].clone())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 5);
    assert_eq!(events[0]["type"], "opened");
    assert_eq!(events[0]["mode"], "reconcile");
    assert!(
        events[1..4]
            .iter()
            .all(|event| event["type"] == "reconciled")
    );
    assert_eq!(events[3]["step_id"], object);
    assert_eq!(events[3]["observed"], *after);
    assert_eq!(events[4]["type"], "closed");
    assert_eq!(events[4]["outcome"], "partial");
}

fn assert_resume_observations(io: &FixtureIo, object: &str) {
    let (last, samples) = io.database_events.split_last().unwrap();
    assert_eq!(samples.len(), 4);
    let observation = DatabaseEvent::Observe(io.database_ids.clone());
    assert!(samples.iter().all(|event| event == &observation));
    assert_eq!(last, &DatabaseEvent::VerifyComplete);
    assert_eq!(
        io.object_batches,
        [vec![object.to_owned()], vec![object.to_owned()]]
    );
    assert_eq!(io.complete_checks, 1);
}

fn assert_final_result(copy: &Path, ledger: &Path) {
    let saved = document::read(&copy.join("result.json")).unwrap();
    assert_eq!(saved["status"], "data_steps_verified");
    assert_eq!(saved["written_steps"], 3);
    assert_eq!(saved["unchanged_databases"], 2);
    assert_eq!(saved["target_ready"], false);
    assert_eq!(saved["restore_success"], false);
    assert_eq!(completion_count(ledger), 1);
}
