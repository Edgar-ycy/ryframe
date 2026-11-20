use super::*;
use std::path::PathBuf;

use crate::recovery::{
    clone::native::{
        json as document,
        target_verify::{guard::Generation, tree},
    },
    fixture_control::portable_path,
};

const PLAN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GENERATION: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub(in crate::recovery::clone::native) const ARTIFACT: &str =
    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

pub(in crate::recovery::clone::native) struct Fixture {
    _temp: tempfile::TempDir,
    pub(in crate::recovery::clone::native) backend: PathBuf,
    pub(in crate::recovery::clone::native) copy: PathBuf,
    initialized: Value,
    export: Value,
}

impl Fixture {
    pub(in crate::recovery::clone::native) fn new() -> Self {
        let managed = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&managed).unwrap();
        let temp = tempfile::tempdir_in(managed).unwrap();
        let backend = portable_path(&fs::canonicalize(temp.path()).unwrap());
        let local = backend.join(".local-tests");
        let copy = local.join("copy");
        let target = local.join("target");
        fs::create_dir_all(&copy).unwrap();
        fs::create_dir(&target).unwrap();
        let initialized_path = target.join("initialized.json");
        let export_path = local.join("export.json");
        document::write_new(&initialized_path, &json!({"target":true})).unwrap();
        document::write_new(&export_path, &json!({"source":true})).unwrap();
        let initialized = common::binding(&initialized_path).unwrap();
        let export = common::binding(&export_path).unwrap();
        document::write_new(
            &copy.join("session.json"),
            &json!({
                "source_export":export, "initialized":initialized,
                "plan_sha256":PLAN, "generation_sha256":GENERATION,
            }),
        )
        .unwrap();
        Self {
            _temp: temp,
            backend,
            copy,
            initialized,
            export,
        }
    }

    pub(in crate::recovery::clone::native) fn run(
        &self,
        mode: Mode,
        execute: impl FnOnce(&mut Session<'_>, &Owner) -> Result<()>,
    ) -> Result<Value> {
        self.run_with_publication(mode, execute, |_| Ok(()))
            .map(|(_, value)| value)
    }

    pub(in crate::recovery::clone::native) fn run_with_publication<T>(
        &self,
        mode: Mode,
        execute: impl FnOnce(&mut Session<'_>, &Owner) -> Result<T>,
        publish: impl FnOnce(&(T, Value)) -> Result<()>,
    ) -> Result<(T, Value)> {
        let owner = Owner::register(&self.backend, &self.copy, &self.export, &self.initialized)?;
        let generation = Generation::acquire_copy(owner)?;
        let result = generation.copy_session().and_then(|(owner, guard, lock)| {
            run(owner, guard, lock, mode, |session| execute(session, owner))
        });
        generation.finish_publishing(result, publish)
    }

    fn ledger(&self) -> PathBuf {
        self.copy.join("ledger")
    }

    fn inspect(&self) -> Value {
        inspect(&self.backend, &self.ledger(), PLAN, GENERATION).unwrap()
    }
}

pub(in crate::recovery::clone::native) fn images(resource: char) -> (Value, Value) {
    let hash = resource.to_string().repeat(64);
    let before = json!({"kind":"object","resource_sha256":hash,
        "exists":false,"bytes":null,"sha256":null,"metadata_sha256":null});
    let after = json!({"kind":"object","resource_sha256":hash,
        "exists":true,"bytes":7,"sha256":ARTIFACT,"metadata_sha256":"d".repeat(64)});
    (before, after)
}

#[test]
fn publishes_the_original_canonical_receipt_only_after_releasing_the_ledger_lock() {
    let fixture = Fixture::new();
    let (before, after) = images('1');
    let observed = fixture
        .run(Mode::New, |session, owner| {
            session.intent("object-first", &before, &after, ARTIFACT)?;
            assert_eq!(fixture.inspect()["status"], "needs_reconciliation");
            assert!(session.confirm("object-first", &after).is_err());
            session.write_once("object-first", &before, &after, ARTIFACT, |verify| verify())?;
            session.confirm("object-first", &after)?;
            session.finish()?;
            assert!(session.confirm("object-first", &after).is_err());
            assert!(
                !fixture
                    .ledger()
                    .join(format!("complete-{}.json", owner.session_id()))
                    .exists()
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(observed["status"], "ledger_evidence_complete");
    assert_eq!(observed["target_ready"], false);
    assert_eq!(observed, fixture.inspect());
    assert!(!fixture.ledger().join("clone.lock").exists());
}

#[test]
fn interrupted_write_requires_explicit_before_reconciliation_before_resume() {
    let fixture = Fixture::new();
    let (before, after) = images('2');
    let initial = fixture.run(Mode::New, |session, _| {
        session.intent("object-first", &before, &after, ARTIFACT)?;
        let failed = session.write_once("object-first", &before, &after, ARTIFACT, |verify| {
            verify()?;
            Err("资源写入结果未知".into())
        });
        assert!(
            session
                .write_once::<()>("object-first", &before, &after, ARTIFACT, |_| {
                    panic!("失败后不能再次发出写入")
                })
                .is_err()
        );
        assert!(session.confirm("object-first", &after).is_err());
        failed
    });
    assert!(initial.is_err());
    assert_eq!(
        fixture.inspect()["steps"]["object-first"]["phase"],
        "unknown"
    );
    let journal = fixture.ledger().join("events.ndjson");
    let raw = fs::read(&journal).unwrap();
    assert!(
        super::super::check_continuation(
            &fixture.backend,
            &fixture.ledger(),
            PLAN,
            GENERATION,
            Mode::Resume,
        )
        .is_err()
    );
    super::super::check_continuation(
        &fixture.backend,
        &fixture.ledger(),
        PLAN,
        GENERATION,
        Mode::Reconcile,
    )
    .unwrap();
    assert!(
        fixture
            .run(Mode::Resume, |_, _| panic!("不能执行未核对的步骤"))
            .is_err()
    );
    assert_eq!(fs::read(&journal).unwrap(), raw);
    fixture
        .run(Mode::Reconcile, |session, _| {
            assert_eq!(
                session.reconcile("object-first", &before)?,
                "reconciled_before"
            );
            assert!(session.finish().is_err());
            Ok(())
        })
        .unwrap();
    super::super::check_continuation(
        &fixture.backend,
        &fixture.ledger(),
        PLAN,
        GENERATION,
        Mode::Resume,
    )
    .unwrap();
    let evidence = fixture
        .run(Mode::Resume, |session, _| {
            session.resume("object-first", &before)?;
            session.write_once("object-first", &before, &after, ARTIFACT, |verify| verify())?;
            session.confirm("object-first", &after)?;
            session.finish()
        })
        .unwrap();
    assert_eq!(evidence["status"], "ledger_evidence_complete");
    assert_eq!(evidence["steps"]["object-first"]["attempt"], 2);
}

#[test]
fn copy_history_uses_the_registered_target_lock_and_releases_it_before_publishing() {
    let fixture = Fixture::new();
    let owner = Owner::register(
        &fixture.backend,
        &fixture.copy,
        &fixture.export,
        &fixture.initialized,
    )
    .unwrap();
    let lock_path = owner.lock_path(Role::Target).to_owned();
    let target = lock_path.parent().unwrap();
    let generation = Generation::acquire_copy(owner).unwrap();
    let (owner, process, record) = generation.copy_session().unwrap();
    assert_eq!(
        owner.verify_lock(Role::Target).unwrap().as_ref(),
        Some(record)
    );
    process
        .check_named(target, Role::Target.guard_name())
        .unwrap();
    let held = tree::protected_snapshot(&fixture.backend, target, Some(&generation)).unwrap();
    assert!(tree::snapshot(&fixture.backend, target, true).is_err());
    assert!(Generation::acquire(&fixture.backend, target).is_err());
    assert!(tree::protected_snapshot(&fixture.backend, &fixture.copy, Some(&generation)).is_err());
    let extra = lock_path.join("unknown.json");
    document::write_new(&extra, &json!({"unexpected":true})).unwrap();
    assert!(tree::protected_snapshot(&fixture.backend, target, Some(&generation)).is_err());
    fs::remove_file(extra).unwrap();
    generation
        .finish_publishing(Ok(()), |_| {
            assert!(!lock_path.exists());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        tree::snapshot(&fixture.backend, target, false).unwrap(),
        held
    );
}

#[test]
fn failed_completion_publication_preserves_the_conflicting_file_and_records_fault() {
    let fixture = Fixture::new();
    let (before, after) = images('3');
    let mut conflict = None;
    let result = fixture.run(Mode::New, |session, owner| {
        session.intent("object-first", &before, &after, ARTIFACT)?;
        session.write_once("object-first", &before, &after, ARTIFACT, |verify| verify())?;
        session.confirm("object-first", &after)?;
        let path = fixture
            .ledger()
            .join(format!("complete-{}.json", owner.session_id()));
        fs::write(&path, b"original conflicting file")?;
        conflict = Some(path);
        session.finish()
    });
    assert!(result.is_err());
    assert_eq!(
        fs::read(conflict.unwrap()).unwrap(),
        b"original conflicting file"
    );
    assert_eq!(fixture.inspect()["status"], "needs_reconciliation");
    assert!(!fixture.ledger().join("clone.lock").exists());
    let journal = fs::read_to_string(fixture.ledger().join("events.ndjson")).unwrap();
    let frame: Value = serde_json::from_str(journal.lines().last().unwrap()).unwrap();
    assert_eq!(
        frame["event"],
        json!({"type":"fault","stage":"receipt_write"})
    );
}

#[test]
fn reconcile_completion_requires_fresh_observations_of_every_existing_step() {
    let fixture = Fixture::new();
    let (one_before, one_after) = images('4');
    let (two_before, two_after) = images('5');
    fixture
        .run(Mode::New, |session, _| {
            session.intent("object-one", &one_before, &one_after, ARTIFACT)?;
            session.write_once("object-one", &one_before, &one_after, ARTIFACT, |verify| {
                verify()
            })?;
            session.confirm("object-one", &one_after)?;
            session.intent("object-two", &two_before, &two_after, ARTIFACT)?;
            session.write_once("object-two", &two_before, &two_after, ARTIFACT, |verify| {
                verify()
            })?;
            session.confirm("object-two", &two_after)
        })
        .unwrap();
    let partial = fixture
        .run(Mode::Reconcile, |session, _| {
            session.reconcile("object-one", &one_after)?;
            assert!(session.finish().is_err());
            Ok(())
        })
        .unwrap();
    assert_eq!(partial["status"], "partial");
    let complete = fixture
        .run(Mode::Reconcile, |session, _| {
            session.reconcile("object-one", &one_after)?;
            session.reconcile("object-two", &two_after)?;
            session.finish()
        })
        .unwrap();
    assert_eq!(complete["status"], "ledger_evidence_complete");
}
