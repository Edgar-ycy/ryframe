use std::{fs, path::Path};

use super::*;
use crate::recovery::clone::native::copy_owner::{Owner, Role};

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
