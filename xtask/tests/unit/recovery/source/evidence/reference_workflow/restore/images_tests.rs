use std::{cell::Cell, fs, path::PathBuf};

use super::super::{Lock, capture_around_write, finish, receipts};
use super::*;

struct Fixture {
    _directory: tempfile::TempDir,
    backend: PathBuf,
    work: PathBuf,
    args: Value,
}

impl Fixture {
    fn new() -> Self {
        let directory = super::super::super::tests::temporary();
        let backend = crate::recovery::fixture_control::portable_path(
            &directory.path().canonicalize().unwrap(),
        );
        let local = backend.join(".local-tests");
        fs::create_dir(&local).unwrap();
        let work = local.join("restore");
        let plan = json!({"id":"restore-case","work_dir":work,"target_side":"base"});
        super::super::super::files::work(&json!({"plan":plan}), true).unwrap();
        let mut bindings = json!({"target_side":"base"});
        for key in [
            "target_plan",
            "backup_receipt",
            "record",
            "runtime_registration",
        ] {
            let path = local.join(format!("{key}.json"));
            native::json::write_new(&path, &json!({"fixture":key})).unwrap();
            bindings[key] = common::binding(&path).unwrap();
        }
        let args = json!({"plan":plan,"command":"restore","copy_id":null,
            "root":null,"receipt":work.join("restore-base.json"),"bindings":bindings});
        Self {
            _directory: directory,
            backend,
            work,
            args,
        }
    }

    fn image(&self, phase: Phase, evidence: &mut Map<String, Value>, matches: bool) -> Result<()> {
        let directory = self.work.join(format!("restore-base-{}", phase.name()));
        record(&self.backend, &directory, phase, evidence, || {
            let value = json!({
                "phase": phase.name(),
                "matches_expected": matches,
                "image": {
                    "databases":{"target_control":{"rows": if phase == Phase::Before { 0 } else { 1 }}},
                    "objects":{"uploads":{"keys":["target/.ryframe-owner"]}},
                    "redis":{"owner":"target"},
                },
            });
            let binding = publish(&self.backend, &directory.join("image.json"), &value)?;
            Ok((matches, binding))
        })
    }
}

#[test]
fn unknown_write_records_observed_failure_after_and_refuses_replay() {
    let fixture = Fixture::new();
    let lock = Lock::claim(&fixture.work).unwrap();
    let started = receipts::start(&fixture.args).unwrap();
    let mut evidence = Map::new();
    let writes = Cell::new(0);
    let result = capture_around_write(
        |phase, evidence| {
            lock.check()?;
            receipts::check_started(&started)?;
            match phase {
                Phase::Before => assert_eq!(writes.get(), 0),
                Phase::FailureAfter => assert_eq!(writes.get(), 1),
                Phase::After => panic!("未知写入后不能采集成功后像"),
            }
            fixture.image(phase, evidence, phase == Phase::Before)
        },
        || {
            lock.check()?;
            receipts::check_started(&started)?;
            assert!(
                fixture
                    .work
                    .join("restore-base-before/image.json")
                    .is_file()
            );
            assert!(Lock::claim(&fixture.work).is_err());
            writes.set(writes.get() + 1);
            Err("数据库写入结果未知".into())
        },
        &mut evidence,
    );
    assert_eq!(writes.get(), 1);
    assert_eq!(
        finish(&started, result.map(|()| Value::Null), evidence)
            .unwrap_err()
            .to_string(),
        "数据库写入结果未知"
    );
    let path = fixture.work.join("restore-base.json");
    let original = fs::read(&path).unwrap();
    let saved = native::json::read(&path).unwrap();
    assert_eq!(saved["status"], "failed");
    assert!(saved.get("result").is_none());
    let images = saved["resource_images"].as_object().unwrap();
    assert_eq!(
        images.keys().map(String::as_str).collect::<Vec<_>>(),
        ["before", "failure-after"]
    );
    let (_, observed) = document::bound_file(
        &fixture.backend,
        &images["failure-after"],
        "参考失败后实际像",
    )
    .unwrap();
    assert_eq!(observed.value["matches_expected"], false);
    assert_eq!(
        observed.value["image"]["databases"]["target_control"]["rows"],
        1
    );
    assert!(!observed.path.with_file_name("failure.json").exists());
    drop(lock);
    assert!(receipts::start(&fixture.args).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn strict_success_mismatch_and_unreadable_failure_image_keep_failure_diagnostics() {
    let fixture = Fixture::new();
    let mut evidence = Map::new();
    assert!(fixture.image(Phase::After, &mut evidence, false).is_err());
    assert!(evidence.contains_key("after"));
    assert!(evidence.contains_key("after-failure"));

    let directory = fixture.work.join("restore-base-failure-after");
    let error = record(
        &fixture.backend,
        &directory,
        Phase::FailureAfter,
        &mut evidence,
        || Err("五桶归属不符".into()),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "五桶归属不符");
    assert!(!evidence.contains_key("failure-after"));
    assert_eq!(
        evidence["failure-after-failure"],
        common::binding(&directory.join("failure.json")).unwrap()
    );
}
