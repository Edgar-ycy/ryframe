use std::{collections::BTreeSet, fs, path::Path};

use super::*;
use crate::recovery::clone::native::{
    copy_ledger::check_continuation,
    copy_owner::{Owner, Role},
    copy_transfer::{publication::Attempt, record::Prepared},
};

fn four_database_declaration(objects: usize) -> Value {
    let mut value = declaration(objects);
    value["databases"] = json!(
        ["shared-control", "shared", "dedicated-a", "dedicated-b"]
            .map(|key| json!({"key":key,"artifact":{"sha256":ARTIFACT}}))
    );
    value
}

fn database_writes(io: &mut FixtureIo, steps: &[Step<'_>]) {
    for step in steps {
        io.bases.get_mut(&step.id).unwrap()["after"]["tables_sha256"] = json!("8".repeat(64));
    }
}

fn ledger_path(owner: &Owner) -> &Path {
    owner.lock_path(Role::Ledger).parent().unwrap()
}

fn completion_count(ledger: &Path) -> usize {
    fs::read_dir(ledger)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| {
            let name = name.to_str().unwrap();
            name.starts_with("complete-") && name.ends_with(".json")
        })
        .count()
}

fn rejected_session(
    fixture: &Fixture,
    mode: Mode,
    execute: impl FnOnce(&mut Session<'_>, &Owner) -> Result<()>,
) {
    let mut ledger = None;
    fixture
        .run(mode, |session, owner| {
            ledger = Some(ledger_path(owner).to_owned());
            execute(session, owner)
        })
        .unwrap_err();
    assert_eq!(completion_count(&ledger.unwrap()), 0);
}

fn stage_databases(
    plan: &Plan<'_>,
    count: usize,
    session: &mut Session<'_>,
    io: &mut FixtureIo,
) -> Result<Map<String, Value>> {
    let mut noops = Map::new();
    for step in &plan.databases[..count] {
        apply(plan, step, session, io, false, &mut noops)?;
    }
    Ok(noops)
}

#[test]
fn mixed_and_all_noop_databases_publish_with_the_exact_unchanged_images() {
    for changed_count in [2, 0] {
        let declaration = four_database_declaration(1);
        let plan = Plan::from_verified(&declaration).unwrap();
        let (changed, unchanged) = plan.databases.split_at(changed_count);
        let mut io = FixtureIo::new(&plan);
        database_writes(&mut io, changed);
        let expected_noops = unchanged
            .iter()
            .map(|step| (step.id.clone(), io.current[&step.id].clone()))
            .collect::<Map<_, _>>();
        let expected_writes = changed
            .iter()
            .chain(&plan.objects)
            .map(|step| step.id.clone())
            .collect::<Vec<_>>();
        let fixture = Fixture::new();
        let mut ledger = None;
        let receipt = fixture
            .run(Mode::New, |session, owner| {
                ledger = Some(ledger_path(owner).to_owned());
                let Outcome::Verified {
                    unchanged_databases,
                } = run(&plan, session, Mode::New, &mut io)?
                else {
                    panic!("真实复制未得到完整收尾");
                };
                assert_eq!(unchanged_databases, expected_noops);
                assert_eq!(io.writes, expected_writes);
                assert_eq!(io.complete_checks, 1);
                Ok(())
            })
            .unwrap();
        assert_eq!(receipt["status"], "ledger_evidence_complete");
        assert_eq!(completion_count(&ledger.unwrap()), 1);
        let steps = receipt["steps"].as_object().unwrap();
        assert_eq!(steps.len(), expected_writes.len());
        for id in expected_writes {
            assert_eq!(steps[&id]["phase"], "confirmed");
            assert_eq!(steps[&id]["attempt"], 1);
        }
        for (id, image) in &expected_noops {
            assert!(!steps.contains_key(id));
            assert_eq!(io.current[id], *image);
        }
        let sample = DatabaseEvent::Observe(io.database_ids.clone());
        assert_eq!(
            io.database_events
                .iter()
                .filter(|event| **event == sample)
                .count(),
            changed_count * 2 + unchanged.len() + 1
        );
        assert_eq!(
            &io.database_events[io.database_events.len() - 3..],
            &[
                DatabaseEvent::WriteObject(plan.objects[0].id.clone()),
                sample,
                DatabaseEvent::VerifyComplete,
            ]
        );
    }
}

fn run_published_copy(
    fixture: &Fixture,
    prepared: &Prepared,
    plan: &Plan<'_>,
    mode: Mode,
    io: &mut FixtureIo,
) -> Result<(Value, Value)> {
    let target_lock = fixture.backend.join(".local-tests/target/initialize.lock");
    let ledger_lock = fixture.copy.join("ledger/clone.lock");
    let attempt = Attempt::new(mode, &fixture.copy);
    let mut published = None;
    let transfer = fixture.run_with_publication(
        mode,
        |session, owner| {
            for (role, lock) in [(Role::Target, &target_lock), (Role::Ledger, &ledger_lock)] {
                assert_eq!(owner.lock_path(role), lock.as_path());
                assert!(lock.is_dir());
            }
            run(plan, session, mode, io)
        },
        |(outcome, ledger)| {
            assert!(!target_lock.try_exists()?);
            assert!(!ledger_lock.try_exists()?);
            published = Some(attempt.publish_bound(
                &fixture.backend,
                &fixture.copy,
                prepared,
                outcome,
                ledger,
            )?);
            Ok(())
        },
    );
    if transfer.is_err() {
        assert!(!target_lock.try_exists()?);
        assert!(!ledger_lock.try_exists()?);
    }
    let (_, ledger) = attempt.failure(&fixture.backend, &fixture.copy, transfer, "transfer")?;
    Ok((published.unwrap(), ledger))
}

#[test]
fn completed_copy_reconciles_and_resumes_without_rewriting_original_result() {
    let fixture = Fixture::new();
    let mut declaration = four_database_declaration(1);
    declaration["pending_target_actions"] = json!([]);
    let prepared = Prepared::from_test_session(&fixture.copy, declaration).unwrap();
    let plan = Plan::from_verified(&prepared.plan).unwrap();
    let mut io = FixtureIo::new(&plan);
    database_writes(&mut io, &plan.databases[..2]);
    let expected_writes = [
        plan.databases[0].id.clone(),
        plan.databases[1].id.clone(),
        plan.objects[0].id.clone(),
    ];
    let ledger = fixture.copy.join("ledger");
    let journal = ledger.join("events.ndjson");
    let (first, first_ledger) =
        run_published_copy(&fixture, &prepared, &plan, Mode::New, &mut io).unwrap();
    assert_eq!(first["status"], "data_steps_verified");
    assert_eq!(first["written_steps"], 3);
    assert_eq!(first["unchanged_databases"], 2);
    assert_eq!(first_ledger["status"], "ledger_evidence_complete");
    assert_eq!(io.writes, expected_writes);
    assert_eq!(io.complete_checks, 1);
    let session = first_ledger["steps"][&expected_writes[0]]["session"]
        .as_str()
        .unwrap();
    let original = [
        fixture.copy.join("result.json"),
        ledger.join(format!("complete-{session}.json")),
    ]
    .map(|path| {
        let bytes = fs::read(&path).unwrap();
        (path, bytes)
    });
    let assert_preserved = |prefix: &[u8]| {
        for (path, bytes) in &original {
            assert_eq!(&fs::read(path).unwrap(), bytes);
        }
        let next = fs::read(&journal).unwrap();
        assert!(next.len() > prefix.len() && next.starts_with(prefix));
        next
    };
    let check = |mode| {
        check_continuation(
            &fixture.backend,
            &ledger,
            prepared.record["plan_sha256"].as_str().unwrap(),
            prepared.record["generation_sha256"].as_str().unwrap(),
            mode,
        )
    };
    let prefix = fs::read(&journal).unwrap();
    let error = check(Mode::Resume).unwrap_err();
    assert!(error.to_string().contains("必须先显式核对"));
    assert_eq!(fs::read(&journal).unwrap(), prefix);
    check(Mode::Reconcile).unwrap();
    let (reconciled, _) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Reconcile, &mut io).unwrap();
    assert_eq!(reconciled["status"], "copy_reconciled");
    assert_eq!(reconciled["remote_writes"], 0);
    assert_eq!(io.writes, expected_writes);
    assert_eq!(io.complete_checks, 1);
    let prefix = assert_preserved(&prefix);

    check(Mode::Resume).unwrap();
    let (mut resumed, completed) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Resume, &mut io).unwrap();
    assert_ne!(
        resumed
            .as_object_mut()
            .unwrap()
            .remove("continuation_receipt")
            .unwrap(),
        reconciled["continuation_receipt"]
    );
    assert_eq!(resumed, first);
    assert_eq!(completed["status"], "ledger_evidence_complete");
    assert_eq!(completion_count(&ledger), 2);
    assert_eq!(
        completed["steps"].as_object().unwrap().len(),
        expected_writes.len()
    );
    assert_eq!(io.writes, expected_writes);
    for id in expected_writes {
        assert_eq!(completed["steps"][&id]["phase"], "confirmed");
        assert_eq!(completed["steps"][&id]["attempt"], 1);
    }
    assert_eq!(io.complete_checks, 2);
    let _ = assert_preserved(&prefix);
}

#[test]
fn lost_database_commit_is_reconciled_and_not_dispatched_again_on_resume() {
    let fixture = Fixture::new();
    let mut declaration = four_database_declaration(1);
    declaration["pending_target_actions"] = json!([]);
    let prepared = Prepared::from_test_session(&fixture.copy, declaration).unwrap();
    let plan = Plan::from_verified(&prepared.plan).unwrap();
    let mut io = FixtureIo::new(&plan);
    database_writes(&mut io, &plan.databases[..2]);
    let first = &plan.databases[0].id;
    let after = io.bases[first]["after"].clone();
    io.stop_at = Some(first.clone());
    io.failed_database_image = Some(after.clone());
    let error = run_published_copy(&fixture, &prepared, &plan, Mode::New, &mut io).unwrap_err();
    assert_eq!(error.to_string(), "复制进程结束，资源写入结果未知");
    assert_eq!(io.writes.as_slice(), std::slice::from_ref(first));
    assert_eq!(io.current[first], after);
    io.stop_at = None;

    let ledger = fixture.copy.join("ledger");
    let result_path = fixture.copy.join("result.json");
    let failure_path = fixture.copy.join("failure.json");
    let paths = [
        &failure_path,
        &ledger.join("events.ndjson"),
        &ledger.join("head.json"),
    ];
    let original = paths.map(|path| fs::read(path).unwrap());
    let failure: Value = serde_json::from_slice(&original[0]).unwrap();
    assert_eq!(failure["phase"], "transfer");
    assert_eq!(failure["status"], "needs_reconciliation");
    assert_eq!(failure["automatic_retry"], false);
    let check = |mode| {
        check_continuation(
            &fixture.backend,
            &ledger,
            prepared.record["plan_sha256"].as_str().unwrap(),
            prepared.record["generation_sha256"].as_str().unwrap(),
            mode,
        )
    };
    let error = check(Mode::Resume).unwrap_err();
    assert!(error.to_string().contains("必须先显式核对"));
    assert_eq!(paths.map(|path| fs::read(path).unwrap()), original);
    assert_eq!(io.writes.as_slice(), std::slice::from_ref(first));

    check(Mode::Reconcile).unwrap();
    let (reconciled, reconciled_ledger) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Reconcile, &mut io).unwrap();
    assert_eq!(reconciled["status"], "copy_reconciled");
    assert_eq!(reconciled["remote_writes"], 0);
    assert_eq!(reconciled["steps"].as_object().unwrap().len(), 1);
    assert_eq!(reconciled["steps"][first], "confirmed");
    assert_eq!(reconciled_ledger["steps"][first]["attempt"], 1);
    let pending = plan
        .all()
        .filter(|step| &step.id != first)
        .map(|step| &step.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(reconciled["pending_steps"], json!(pending));
    assert_eq!(io.writes.as_slice(), std::slice::from_ref(first));
    assert_eq!(io.complete_checks, 0);
    assert!(!result_path.exists());
    assert_eq!(completion_count(&ledger), 0);
    assert_eq!(fs::read(&failure_path).unwrap(), original[0]);

    check(Mode::Resume).unwrap();
    let (_, completed) =
        run_published_copy(&fixture, &prepared, &plan, Mode::Resume, &mut io).unwrap();
    let second = &plan.databases[1].id;
    let object = &plan.objects[0].id;
    assert_eq!(io.writes, [first.clone(), second.clone(), object.clone()]);
    assert_eq!(io.complete_checks, 1);
    assert_eq!(completed["status"], "ledger_evidence_complete");
    assert_eq!(completed["steps"][first]["phase"], "confirmed");
    assert_eq!(completed["steps"][first]["attempt"], 1);
    let saved: Value = serde_json::from_slice(&fs::read(result_path).unwrap()).unwrap();
    assert_eq!(saved["status"], "data_steps_verified");
    assert_eq!(saved["written_steps"], 3);
    assert_eq!(saved["target_ready"], false);
    assert_eq!(saved["restore_success"], false);
    assert_eq!(fs::read(&failure_path).unwrap(), original[0]);
}

fn reject_incomplete_phase(
    plan: &Plan<'_>,
    session: &mut Session<'_>,
    io: &mut FixtureIo,
    owner: &Owner,
    phase: &str,
) -> Result<()> {
    let id = &plan.databases[3].id;
    let before = session.steps()?;
    assert_eq!(before.len(), 4);
    for step in &plan.databases[..3] {
        assert_eq!(before[&step.id]["phase"], "confirmed");
    }
    assert_eq!(before[id]["phase"], phase);
    assert_eq!(io.current[id], before[id]["after"]);
    assert_eq!(io.writes, io.database_ids);
    let journal = ledger_path(owner).join("events.ndjson");
    let journal_bytes = fs::read(&journal)?;
    io.database_events.clear();
    let error = finish(plan, session, io, &Map::new()).unwrap_err();
    assert!(error.to_string().contains("完整后像未确认"));
    assert_eq!(session.steps()?, before);
    assert_eq!(fs::read(journal)?, journal_bytes);
    assert_eq!(io.writes, io.database_ids);
    assert_eq!(
        io.database_events,
        [DatabaseEvent::Observe(io.database_ids.clone())]
    );
    assert_eq!(io.complete_checks, 0);
    Err(error)
}

#[test]
fn matching_after_images_do_not_complete_unknown_or_reconciled_before_intents() {
    for phase in ["unknown", "reconciled_before"] {
        let declaration = four_database_declaration(0);
        let plan = Plan::from_verified(&declaration).unwrap();
        let mut io = FixtureIo::new(&plan);
        database_writes(&mut io, &plan.databases);
        let last = &plan.databases[3].id;
        let after = io.bases[last]["after"].clone();
        io.stop_at = Some(last.clone());
        io.failed_database_image = (phase == "unknown").then_some(after.clone());
        let fixture = Fixture::new();
        rejected_session(&fixture, Mode::New, |session, owner| {
            let error = run(&plan, session, Mode::New, &mut io).err().unwrap();
            assert!(error.to_string().contains("资源写入结果未知"));
            if phase == "unknown" {
                reject_incomplete_phase(&plan, session, &mut io, owner, phase)
            } else {
                Err(error)
            }
        });
        if phase == "reconciled_before" {
            fixture
                .run(Mode::Reconcile, |session, _| {
                    let Outcome::Reconciled(receipt) =
                        run(&plan, session, Mode::Reconcile, &mut io)?
                    else {
                        panic!("显式对账必须返回只读结果");
                    };
                    assert_eq!(receipt["steps"][last], phase);
                    assert_eq!(receipt["remote_writes"], 0);
                    Ok(())
                })
                .unwrap();
            io.stop_at = None;
            io.current.insert(last.clone(), after);
            rejected_session(&fixture, Mode::Resume, |session, owner| {
                reject_incomplete_phase(&plan, session, &mut io, owner, phase)
            });
        }
    }
}

#[test]
fn final_database_batch_failures_preserve_the_ledger_and_block_complete_verification() {
    for fault in [
        "missing",
        "identity",
        "owner",
        "postimage",
        "checkpoint",
        "basis",
        "ledger",
    ] {
        let declaration = four_database_declaration(0);
        let plan = Plan::from_verified(&declaration).unwrap();
        let mut io = FixtureIo::new(&plan);
        database_writes(&mut io, &plan.databases);
        let id = &plan.databases[1].id;
        let fixture = Fixture::new();
        rejected_session(&fixture, Mode::New, |session, owner| {
            let noops = stage_databases(&plan, 4, session, &mut io)?;
            assert!(noops.is_empty());
            let before = session.steps()?;
            assert_eq!(before.len(), 4);
            assert!(before.values().all(|row| row["phase"] == "confirmed"));
            let journal = ledger_path(owner).join("events.ndjson");
            let head = ledger_path(owner).join("head.json");
            let journal_bytes = fs::read(&journal)?;
            let mut head_bytes = fs::read(&head)?;
            io.database_events.clear();
            let expected_error = match fault {
                "missing" => {
                    io.omitted_database = Some(id.clone());
                    "完整资源观察缺少"
                }
                "identity" => {
                    io.current.get_mut(id).unwrap()["resource_sha256"] = json!("9".repeat(64));
                    "物理资源身份"
                }
                "owner" => {
                    io.current.get_mut(id).unwrap()["preserved_sha256"] = json!("9".repeat(64));
                    "完整后像"
                }
                "postimage" => {
                    io.current.get_mut(id).unwrap()["tables_sha256"] = json!("9".repeat(64));
                    "完整后像"
                }
                "checkpoint" => {
                    io.database_fault = Some(DatabaseFault::Checkpoint);
                    "数据库观察后代次校验失败"
                }
                "basis" => {
                    io.database_fault = Some(DatabaseFault::Basis(id.clone()));
                    "冻结步骤前后像变化"
                }
                "ledger" => {
                    head_bytes.push(b'\n');
                    io.database_fault = Some(DatabaseFault::Ledger(head.clone()));
                    "账本文件身份或原始字节发生变化"
                }
                _ => unreachable!(),
            };
            let error = finish(&plan, session, &mut io, &noops).unwrap_err();
            assert!(
                error.to_string().contains(expected_error),
                "{fault}: {error}"
            );
            assert_eq!(fs::read(journal)?, journal_bytes);
            assert_eq!(fs::read(head)?, head_bytes);
            if fault != "ledger" {
                assert_eq!(session.steps()?, before);
            }
            assert_eq!(io.writes, io.database_ids);
            assert_eq!(
                io.database_events,
                [DatabaseEvent::Observe(io.database_ids.clone())]
            );
            assert_eq!(io.complete_checks, 0);
            Err(error)
        });
    }
}

#[test]
fn missing_overlapping_or_unproven_noop_images_cannot_complete() {
    for fault in ["missing", "overlap", "changed-image", "unequal-basis"] {
        let declaration = four_database_declaration(0);
        let plan = Plan::from_verified(&declaration).unwrap();
        let mut io = FixtureIo::new(&plan);
        database_writes(&mut io, &plan.databases[..3]);
        let last = &plan.databases[3];
        if fault == "unequal-basis" {
            database_writes(&mut io, std::slice::from_ref(last));
        }
        let fixture = Fixture::new();
        rejected_session(&fixture, Mode::New, |session, _| {
            assert!(stage_databases(&plan, 3, session, &mut io)?.is_empty());
            let before = session.steps()?;
            assert_eq!(before.len(), 3);
            assert!(!before.contains_key(&last.id));
            let mut noops = Map::new();
            if fault != "unequal-basis" {
                apply(&plan, last, session, &mut io, false, &mut noops)?;
            }
            match fault {
                "missing" => noops.clear(),
                "overlap" => {
                    let confirmed = &plan.databases[0].id;
                    noops.clear();
                    noops.insert(confirmed.clone(), io.bases[confirmed]["before"].clone());
                }
                "changed-image" => {
                    noops.get_mut(&last.id).unwrap()["tables_sha256"] = json!("9".repeat(64));
                }
                "unequal-basis" => {
                    noops.insert(last.id.clone(), io.bases[&last.id]["before"].clone());
                }
                _ => unreachable!(),
            }
            io.database_events.clear();
            let error = finish(&plan, session, &mut io, &noops).unwrap_err();
            let expected = if matches!(fault, "missing" | "overlap") {
                assert!(io.database_events.is_empty());
                "未确认或未只读核验"
            } else {
                assert_eq!(
                    io.database_events,
                    [DatabaseEvent::Observe(io.database_ids.clone())]
                );
                "完整后像未确认"
            };
            assert!(error.to_string().contains(expected), "{fault}: {error}");
            assert_eq!(session.steps()?, before);
            assert_eq!(io.writes, io.database_ids[..3]);
            assert_eq!(io.complete_checks, 0);
            Err(error)
        });
    }
}

#[test]
fn independent_complete_verification_failure_never_publishes_completion() {
    let declaration = four_database_declaration(1);
    let plan = Plan::from_verified(&declaration).unwrap();
    let mut io = FixtureIo::new(&plan);
    database_writes(&mut io, &plan.databases[..2]);
    io.fail_verify_complete = true;
    let fixture = Fixture::new();
    rejected_session(&fixture, Mode::New, |session, _| {
        let error = run(&plan, session, Mode::New, &mut io).err().unwrap();
        assert!(error.to_string().contains("独立完整核验失败"));
        let steps = session.steps()?;
        assert_eq!(steps.len(), 3);
        assert!(steps.values().all(|row| row["phase"] == "confirmed"));
        assert_eq!(
            io.writes,
            plan.databases[..2]
                .iter()
                .chain(&plan.objects)
                .map(|step| step.id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(io.complete_checks, 1);
        assert_eq!(
            &io.database_events[io.database_events.len() - 2..],
            &[
                DatabaseEvent::Observe(io.database_ids.clone()),
                DatabaseEvent::VerifyComplete
            ]
        );
        Err(error)
    });
}
