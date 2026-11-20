use super::*;
use std::{fs::OpenOptions, io::Write, path::PathBuf};

use crate::recovery::clone::native::copy_ledger::session::tests::{ARTIFACT, Fixture, images};

#[path = "steps_database.rs"]
mod database;
#[path = "steps_finish.rs"]
mod finish_cases;
#[path = "steps_resume_object.rs"]
mod resume_object;
#[path = "steps_resume_orphan.rs"]
mod resume_orphan;

fn declaration(object_count: usize) -> Value {
    json!({
        "databases": [{"key":"control", "artifact":{"sha256":ARTIFACT}}],
        "objects": (0..object_count).map(|index| json!({
            "bucket":"uploads", "target_key":format!("target-{index}"),
            "artifact":{"sha256":ARTIFACT},
        })).collect::<Vec<_>>(),
    })
}

struct FixtureIo {
    current: Map<String, Value>,
    bases: Map<String, Value>,
    writes: Vec<String>,
    object_batches: Vec<Vec<String>>,
    database_ids: Vec<String>,
    database_events: Vec<DatabaseEvent>,
    database_evidence: Map<String, Value>,
    database_fault: Option<DatabaseFault>,
    omitted_database: Option<String>,
    stop_at: Option<String>,
    committed_write_image: Option<Value>,
    resume_target_usable: bool,
    complete_checks: usize,
    fail_verify_complete: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum DatabaseEvent {
    Observe(Vec<String>),
    Write(String),
    WriteObject(String),
    VerifyComplete,
}

enum DatabaseFault {
    Checkpoint,
    Basis(String),
    Ledger(PathBuf),
    Evidence(String),
}

impl FixtureIo {
    fn new(plan: &Plan<'_>) -> Self {
        let mut current = Map::new();
        let mut bases = Map::new();
        for (index, step) in plan.all().enumerate() {
            let hash = format!("{index:064x}");
            let (before, after) = if step.kind == Kind::Database {
                let initial = json!({"kind":"database","resource_sha256":hash,
                    "schema_sha256":"d".repeat(64), "tables_sha256":"e".repeat(64),
                    "preserved_sha256":"f".repeat(64)});
                (initial.clone(), initial)
            } else {
                images(char::from_digit(index.try_into().unwrap(), 16).unwrap())
            };
            current.insert(step.id.clone(), before.clone());
            bases.insert(step.id.clone(), json!({"before":before,"after":after}));
        }
        Self {
            current,
            bases,
            writes: Vec::new(),
            object_batches: Vec::new(),
            database_ids: plan.databases.iter().map(|step| step.id.clone()).collect(),
            database_events: Vec::new(),
            database_evidence: Map::new(),
            database_fault: None,
            omitted_database: None,
            stop_at: None,
            committed_write_image: None,
            resume_target_usable: true,
            complete_checks: 0,
            fail_verify_complete: false,
        }
    }

    fn snapshot<'a>(
        &self,
        steps: impl Iterator<Item = &'a Step<'a>>,
    ) -> Observation<Map<String, Value>> {
        let images = steps
            .map(|step| (step.id.clone(), self.current[&step.id].clone()))
            .collect::<Map<_, _>>();
        Observation {
            evidence: images.clone(),
            images,
        }
    }
}

impl CopyIo for FixtureIo {
    type Evidence = Map<String, Value>;

    fn checkpoint(&mut self, _: Option<&Step<'_>>) -> Result<()> {
        if !matches!(self.database_events.last(), Some(DatabaseEvent::Observe(_))) {
            return Ok(());
        }
        match self.database_fault.take() {
            Some(DatabaseFault::Checkpoint) => return Err("测试：数据库观察后代次校验失败".into()),
            Some(DatabaseFault::Basis(id)) => {
                self.bases.get_mut(&id).unwrap()["after"]["tables_sha256"] = json!("9".repeat(64));
            }
            Some(DatabaseFault::Ledger(path)) => {
                OpenOptions::new()
                    .append(true)
                    .open(path)?
                    .write_all(b"\n")?;
            }
            Some(DatabaseFault::Evidence(id)) => {
                self.database_evidence.get_mut(&id).unwrap()["tables_sha256"] =
                    json!("9".repeat(64));
            }
            None => {}
        }
        Ok(())
    }

    fn check_resume_target(&mut self) -> Result<()> {
        if !self.resume_target_usable {
            return Err("目标 Redis 缺少初始化标记".into());
        }
        Ok(())
    }

    fn basis(&mut self, step: &Step<'_>) -> Result<Basis> {
        Ok(Basis {
            before: self.bases[&step.id]["before"].clone(),
            after: self.bases[&step.id]["after"].clone(),
        })
    }

    fn observe_databases(&mut self, all: &[Step<'_>]) -> Result<Observation<Self::Evidence>> {
        let ids = all.iter().map(|step| step.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids, self.database_ids);
        let mut observed = self.snapshot(all.iter());
        if let Some(id) = self.omitted_database.take() {
            assert!(observed.images.remove(&id).is_some());
            assert!(observed.evidence.remove(&id).is_some());
        }
        self.database_evidence = observed.evidence.clone();
        self.database_events.push(DatabaseEvent::Observe(ids));
        Ok(observed)
    }

    fn observe_objects(&mut self, selected: &[&Step<'_>]) -> Result<Observation<Self::Evidence>> {
        self.object_batches
            .push(selected.iter().map(|step| step.id.clone()).collect());
        Ok(self.snapshot(selected.iter().copied()))
    }

    fn check_observation(&mut self, observed: &Observation<Self::Evidence>) -> Result<()> {
        assert_eq!(observed.images, observed.evidence);
        if observed
            .images
            .values()
            .any(|image| image["kind"] == "database")
            && observed.evidence != self.database_evidence
        {
            return Err("测试：数据库原观察证据变化".into());
        }
        Ok(())
    }

    fn write(&mut self, step: &Step<'_>, session: &mut Session<'_>, basis: &Basis) -> Result<()> {
        session.write_once(
            &step.id,
            &basis.before,
            &basis.after,
            step.artifact,
            |check| {
                check()?;
                self.writes.push(step.id.clone());
                if step.kind == Kind::Database {
                    self.database_events
                        .push(DatabaseEvent::Write(step.id.clone()));
                } else {
                    self.database_events
                        .push(DatabaseEvent::WriteObject(step.id.clone()));
                }
                if self.stop_at.as_ref() == Some(&step.id) {
                    if let Some(image) = self.committed_write_image.take() {
                        self.current.insert(step.id.clone(), image);
                    }
                    return Err("复制进程结束，资源写入结果未知".into());
                }
                self.current.insert(step.id.clone(), basis.after.clone());
                check()
            },
        )
    }

    fn verify_complete(&mut self) -> Result<()> {
        self.complete_checks += 1;
        self.database_events.push(DatabaseEvent::VerifyComplete);
        if self.fail_verify_complete {
            return Err("测试：独立完整核验失败".into());
        }
        Ok(())
    }
}

#[test]
fn rejects_unregistered_drift_before_appending_a_reconcile_batch_and_resumes_only_known_before() {
    let fixture = Fixture::new();
    let declaration = declaration(9);
    let plan = Plan::from_verified(&declaration).unwrap();
    let mut io = FixtureIo::new(&plan);
    let ids: Vec<_> = plan.objects.iter().map(|step| step.id.clone()).collect();
    let expected_batches = ids.chunks(4).map(<[String]>::to_vec).collect::<Vec<_>>();
    let database = &plan.databases[0].id;
    io.stop_at = Some(ids[1].clone());
    assert!(
        fixture
            .run(Mode::New, |session, _| {
                run(&plan, session, Mode::New, &mut io).map(|_| ())
            })
            .is_err()
    );
    assert_eq!(io.writes, ids[..2]);
    let batches_before_resume = io.object_batches.len();
    assert!(
        fixture
            .run(Mode::Resume, |session, _| {
                run(&plan, session, Mode::Resume, &mut io).map(|_| ())
            })
            .is_err()
    );
    assert_eq!(io.object_batches.len(), batches_before_resume);
    assert_eq!(io.writes, ids[..2]);

    let drift = io.bases[&ids[2]]["after"].clone();
    let before = io.current.insert(ids[2].clone(), drift).unwrap();
    assert!(
        fixture
            .run(Mode::Reconcile, |session, _| {
                run(&plan, session, Mode::Reconcile, &mut io).map(|_| ())
            })
            .is_err()
    );
    io.current.insert(ids[2].clone(), before);

    let batches_before_reconcile = io.object_batches.len();
    fixture
        .run(Mode::Reconcile, |session, _| {
            assert_eq!(session.steps()?[&ids[1]]["phase"], "unknown");
            let Outcome::Reconciled(receipt) = run(&plan, session, Mode::Reconcile, &mut io)?
            else {
                panic!("显式核对必须返回只读核对结果");
            };
            assert_eq!(receipt["status"], "copy_reconciled");
            assert_eq!(receipt["steps"][&ids[0]], "confirmed");
            assert_eq!(receipt["steps"][&ids[1]], "reconciled_before");
            assert_eq!(receipt["pending_steps"].as_array().unwrap().len(), 8);
            assert_eq!(receipt["remote_writes"], 0);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        &io.object_batches[batches_before_reconcile..],
        expected_batches.as_slice(),
    );
    assert_eq!(io.writes, ids[..2]);

    io.stop_at = None;
    let receipt = fixture
        .run(Mode::Resume, |session, _| {
            let Outcome::Verified {
                unchanged_databases,
            } = run(&plan, session, Mode::Resume, &mut io)?
            else {
                panic!("完整复制必须通过收尾核验");
            };
            assert_eq!(unchanged_databases.len(), 1);
            assert_eq!(unchanged_databases[database], io.current[database]);
            Ok(())
        })
        .unwrap();
    assert_eq!(receipt["status"], "ledger_evidence_complete");
    assert_eq!(io.writes, [ids[..2].to_vec(), ids[1..].to_vec()].concat());
    assert_eq!(io.complete_checks, 1);
    assert_eq!(
        &io.object_batches[io.object_batches.len() - expected_batches.len()..],
        expected_batches.as_slice(),
    );
}

#[test]
fn rejects_resume_without_target_redis_before_observing_or_writing_steps() {
    let fixture = Fixture::new();
    let declaration = declaration(3);
    let plan = Plan::from_verified(&declaration).unwrap();
    let mut io = FixtureIo::new(&plan);
    let first = &plan.objects[0].id;
    let interrupted = &plan.objects[1].id;
    let pending = &plan.objects[2].id;
    io.stop_at = Some(interrupted.clone());
    assert!(
        fixture
            .run(Mode::New, |session, _| {
                run(&plan, session, Mode::New, &mut io).map(|_| ())
            })
            .is_err()
    );

    io.stop_at = None;
    io.resume_target_usable = false;
    fixture
        .run(Mode::Reconcile, |session, _| {
            run(&plan, session, Mode::Reconcile, &mut io).map(|_| ())
        })
        .unwrap();
    let previous_batches = io.object_batches.clone();

    let failure = fixture
        .run(Mode::Resume, |session, _| {
            let before = session.steps()?;
            assert_eq!(before[first]["phase"], "confirmed");
            assert_eq!(before[interrupted]["phase"], "reconciled_before");
            assert!(!before.contains_key(pending));
            let result = run(&plan, session, Mode::Resume, &mut io);
            assert_eq!(session.steps()?, before);
            result.map(|_| ())
        })
        .unwrap_err();
    assert_eq!(failure.to_string(), "目标 Redis 缺少初始化标记");
    assert_eq!(io.object_batches, previous_batches);
    assert_eq!(io.writes.as_slice(), [first.clone(), interrupted.clone()]);
    assert_eq!(io.complete_checks, 0);
}

#[test]
fn independently_observes_the_complete_no_write_plan_before_closing_as_partial() {
    let fixture = Fixture::new();
    let declaration = declaration(0);
    let plan = Plan::from_verified(&declaration).unwrap();
    let mut io = FixtureIo::new(&plan);
    let receipt = fixture
        .run(Mode::New, |session, _| {
            let Outcome::Verified {
                unchanged_databases,
            } = run(&plan, session, Mode::New, &mut io)?
            else {
                panic!("收尾仍须验全量无写入计划");
            };
            assert_eq!(unchanged_databases.len(), 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(receipt["status"], "partial");
    assert_eq!(io.complete_checks, 1);
    assert!(io.writes.is_empty());
}
