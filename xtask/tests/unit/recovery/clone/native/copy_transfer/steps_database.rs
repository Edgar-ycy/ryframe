use std::{collections::BTreeSet, fs};

use super::*;
use crate::recovery::clone::native::copy_owner::Role;

fn declaration() -> Value {
    let databases = ["shared-control", "shared", "dedicated-a", "dedicated-b"]
        .map(|key| json!({"key":key,"artifact":{"sha256":ARTIFACT}}));
    json!({
        "databases": databases,
        "objects": [],
    })
}

fn four_databases(plan: &Plan<'_>) -> FixtureIo {
    let mut io = FixtureIo::new(plan);
    for step in &plan.databases {
        io.bases.get_mut(&step.id).unwrap()["after"]["tables_sha256"] = json!("8".repeat(64));
    }
    assert_eq!(io.database_ids.len(), 4);
    io
}

fn interrupt_second(fixture: &Fixture, plan: &Plan<'_>, io: &mut FixtureIo) {
    io.stop_at = Some(plan.databases[1].id.clone());
    let error = fixture
        .run(Mode::New, |session, _| {
            run(plan, session, Mode::New, io).map(|_| ())
        })
        .unwrap_err();
    assert!(error.to_string().contains("资源写入结果未知"));
    assert_eq!(io.writes, io.database_ids[..2]);
    assert_eq!(
        io.database_events,
        [
            DatabaseEvent::Observe(io.database_ids.clone()),
            DatabaseEvent::Write(io.database_ids[0].clone()),
            DatabaseEvent::Observe(io.database_ids.clone()),
            DatabaseEvent::Observe(io.database_ids.clone()),
            DatabaseEvent::Write(io.database_ids[1].clone()),
        ]
    );
    assert_eq!(io.complete_checks, 0);
    io.stop_at = None;
    io.database_events.clear();
}

fn reconcile(plan: &Plan<'_>, session: &mut Session<'_>, io: &mut FixtureIo) -> Result<Value> {
    let Outcome::Reconciled(receipt) = run(plan, session, Mode::Reconcile, io)? else {
        panic!("四库显式对账必须返回只读结果");
    };
    assert_eq!(receipt["remote_writes"], 0);
    assert_eq!(receipt["target_ready"], false);
    assert_eq!(receipt["restore_success"], false);
    Ok(receipt)
}

#[test]
fn pending_and_confirmed_database_batches_only_read_the_required_full_vectors() {
    let declared = declaration();
    let plan = Plan::from_verified(&declared).unwrap();
    let mut io = four_databases(&plan);
    let fixture = Fixture::new();
    let error = fixture
        .run(Mode::New, |_, _| Err("测试：写入意图前停止".into()))
        .unwrap_err();
    assert!(error.to_string().contains("写入意图前停止"));

    fixture
        .run(Mode::Reconcile, |session, _| {
            let before = session.steps()?;
            assert!(before.is_empty());
            let receipt = reconcile(&plan, session, &mut io)?;
            assert_eq!(receipt["status"], "copy_reconciled");
            assert_eq!(receipt["steps"], json!({}));
            assert_eq!(
                receipt["pending_steps"],
                json!(io.database_ids.iter().collect::<BTreeSet<_>>())
            );
            assert_eq!(session.steps()?, before);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        io.database_events,
        [DatabaseEvent::Observe(io.database_ids.clone())]
    );
    assert!(io.writes.is_empty());

    io.database_events.clear();
    fixture
        .run(Mode::Resume, |session, _| {
            let before = session.steps()?;
            assert!(
                check_batches(&plan, session, &mut io, Check::Confirmed, &Map::new())?.is_empty()
            );
            assert_eq!(session.steps()?, before);
            Ok(())
        })
        .unwrap();
    assert!(io.database_events.is_empty());
    assert!(io.writes.is_empty());

    let fixture = Fixture::new();
    interrupt_second(&fixture, &plan, &mut io);
    fixture
        .run(Mode::Reconcile, |session, _| {
            assert_eq!(
                reconcile(&plan, session, &mut io)?["status"],
                "copy_reconciled"
            );
            Ok(())
        })
        .unwrap();
    io.database_events.clear();
    fixture
        .run(Mode::Resume, |session, _| {
            let before = session.steps()?;
            assert_eq!(before.len(), 2);
            assert_eq!(before[&plan.databases[0].id]["phase"], "confirmed");
            assert_eq!(before[&plan.databases[1].id]["phase"], "reconciled_before");
            assert!(
                check_batches(&plan, session, &mut io, Check::Confirmed, &Map::new())?.is_empty()
            );
            assert_eq!(session.steps()?, before);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        io.database_events,
        [DatabaseEvent::Observe(io.database_ids.clone())]
    );
    assert_eq!(io.writes, io.database_ids[..2]);
}

#[test]
fn mixed_database_intents_are_reconciled_from_one_full_vector_without_replay() {
    for phase in ["reconciled_before", "confirmed", "mismatch"] {
        let declared = declaration();
        let plan = Plan::from_verified(&declared).unwrap();
        let mut io = four_databases(&plan);
        let first = &plan.databases[0].id;
        let second = &plan.databases[1].id;
        let mut image = io.bases[second]["after"].clone();
        if phase == "mismatch" {
            image["tables_sha256"] = json!("9".repeat(64));
        }
        io.failed_database_image = (phase != "reconciled_before").then_some(image);
        let fixture = Fixture::new();
        interrupt_second(&fixture, &plan, &mut io);

        fixture
            .run(Mode::Reconcile, |session, owner| {
                let before = session.steps()?;
                assert_eq!(before.len(), 2);
                assert_eq!(before[first]["phase"], "confirmed");
                assert_eq!(before[second]["phase"], "unknown");
                let receipt = reconcile(&plan, session, &mut io)?;
                let status = if phase == "mismatch" {
                    "needs_reconciliation"
                } else {
                    "copy_reconciled"
                };
                assert_eq!(receipt["status"], status);
                assert_eq!(receipt["steps"].as_object().unwrap().len(), 2);
                assert_eq!(receipt["steps"][first], "confirmed");
                assert_eq!(receipt["steps"][second], phase);
                assert_eq!(
                    receipt["pending_steps"],
                    json!(io.database_ids[2..].iter().collect::<BTreeSet<_>>())
                );
                let mut expected = before;
                for step in expected.values_mut() {
                    step["session"] = json!(owner.session_id());
                }
                expected.get_mut(second).unwrap()["phase"] = json!(phase);
                assert_eq!(session.steps()?, expected);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            io.database_events,
            [DatabaseEvent::Observe(io.database_ids.clone())],
            "{phase}"
        );
        assert_eq!(io.writes, io.database_ids[..2]);
    }
}

#[test]
fn database_window_failure_blocks_all_classification_and_preserves_the_original_steps() {
    for case in ["unregistered", "checkpoint", "basis", "ledger", "evidence"] {
        let declared = declaration();
        let plan = Plan::from_verified(&declared).unwrap();
        let mut io = four_databases(&plan);
        let first = &plan.databases[0].id;
        let second = &plan.databases[1].id;
        let pending = &plan.databases[2].id;
        let fixture = Fixture::new();
        interrupt_second(&fixture, &plan, &mut io);
        fixture
            .run(Mode::Reconcile, |session, owner| {
                let ledger = owner.lock_path(Role::Ledger).parent().unwrap();
                let journal = ledger.join("events.ndjson");
                let head = ledger.join("head.json");
                let before_journal = fs::read(&journal)?;
                let mut expected_head = fs::read(&head)?;
                let before = session.steps()?;
                assert_eq!(before[first]["phase"], "confirmed");
                assert_eq!(before[second]["phase"], "unknown");
                assert!(!before.contains_key(pending));

                let expected_error = match case {
                    "unregistered" => {
                        io.current
                            .insert(pending.clone(), io.bases[pending]["after"].clone());
                        "未登记写入"
                    }
                    "checkpoint" => {
                        io.database_fault = Some(DatabaseFault::Checkpoint);
                        "数据库观察后代次校验失败"
                    }
                    "basis" => {
                        io.database_fault = Some(DatabaseFault::Basis(first.clone()));
                        "冻结步骤前后像变化"
                    }
                    "ledger" => {
                        io.database_fault = Some(DatabaseFault::Ledger(head.clone()));
                        expected_head.push(b'\n');
                        "账本文件身份或原始字节发生变化"
                    }
                    "evidence" => {
                        io.database_fault = Some(DatabaseFault::Evidence(first.clone()));
                        "数据库原观察证据变化"
                    }
                    _ => unreachable!(),
                };
                let error = reconcile(&plan, session, &mut io).unwrap_err();
                assert!(
                    error.to_string().contains(expected_error),
                    "{case}: {error}"
                );
                assert_eq!(fs::read(&journal)?, before_journal);
                assert_eq!(fs::read(&head)?, expected_head);
                if case != "ledger" {
                    assert_eq!(session.steps()?, before);
                }
                Err(error)
            })
            .unwrap_err();
        assert_eq!(
            io.database_events,
            [DatabaseEvent::Observe(io.database_ids.clone())],
            "{case}"
        );
        assert_eq!(io.writes, io.database_ids[..2]);
    }
}

#[test]
fn database_writes_each_have_fresh_before_and_after_vectors_before_final_observation() {
    let declared = declaration();
    let plan = Plan::from_verified(&declared).unwrap();
    let mut io = four_databases(&plan);
    let fixture = Fixture::new();
    let receipt = fixture
        .run(Mode::New, |session, _| {
            let Outcome::Verified {
                unchanged_databases,
            } = run(&plan, session, Mode::New, &mut io)?
            else {
                panic!("四库写入必须经过全量收尾核验");
            };
            assert!(unchanged_databases.is_empty());
            let confirmed = session.steps()?;
            for id in &io.database_ids {
                assert_eq!(confirmed[id]["phase"], "confirmed");
                assert_eq!(confirmed[id]["attempt"], 1);
                assert_eq!(io.current[id], io.bases[id]["after"]);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(receipt["status"], "ledger_evidence_complete");
    assert_eq!(io.writes, io.database_ids);
    assert_eq!(io.complete_checks, 1);
    let sample = || DatabaseEvent::Observe(io.database_ids.clone());
    let mut expected = io
        .database_ids
        .iter()
        .flat_map(|id| [sample(), DatabaseEvent::Write(id.clone()), sample()])
        .collect::<Vec<_>>();
    expected.extend([sample(), DatabaseEvent::VerifyComplete]);
    assert_eq!(io.database_events, expected);
}
