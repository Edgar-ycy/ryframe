use std::{cell::Cell, fs, path::PathBuf};

use super::super::{Lock, capture_around_write, finish, model, receipts};
use super::*;

struct Fixture {
    _directory: tempfile::TempDir,
    backend: PathBuf,
    work: PathBuf,
    args: Value,
}

impl Fixture {
    fn new(side: &str) -> Self {
        let directory = super::super::super::tests::temporary();
        let backend = crate::recovery::fixture_control::portable_path(
            &directory.path().canonicalize().unwrap(),
        );
        let local = backend.join(".local-tests");
        fs::create_dir(&local).unwrap();
        let work = local.join("restore");
        let plan = json!({"id":"restore-case","work_dir":work,"target_side":side});
        super::super::super::files::work(&json!({"plan":plan}), true).unwrap();
        let mut bindings = json!({"target_side":side});
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
            "root":null,"receipt":work.join(format!("restore-{side}.json")),"bindings":bindings});
        Self {
            _directory: directory,
            backend,
            work,
            args,
        }
    }

    fn image(&self, phase: Phase, evidence: &mut Map<String, Value>, matches: bool) -> Result<()> {
        let side = self.args["plan"]["target_side"].as_str().unwrap();
        let directory = self.work.join(format!("restore-{side}-{}", phase.name()));
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
    let fixture = Fixture::new("base");
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
    let fixture = Fixture::new("base");
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

#[test]
fn successful_copy_receipts_keep_the_selected_side_and_exact_input_bindings() {
    for side in ["base", "candidate"] {
        let fixture = Fixture::new(side);
        let lock = Lock::claim(&fixture.work).unwrap();
        let started = receipts::start(&fixture.args).unwrap();
        let mut evidence = Map::new();
        let writes = Cell::new(0);
        capture_around_write(
            |phase, evidence| {
                lock.check()?;
                receipts::check_started(&started)?;
                assert!(phase != Phase::FailureAfter);
                assert_eq!(writes.get(), usize::from(phase == Phase::After));
                fixture.image(phase, evidence, true)
            },
            || {
                lock.check()?;
                writes.set(writes.get() + 1);
                Ok(())
            },
            &mut evidence,
        )
        .unwrap();
        let bindings = &fixture.args["bindings"];
        let runtime = json!({
            "registration":bindings["runtime_registration"],
            "target_plan":bindings["target_plan"],
        });
        let result = model::restore_result(&json!({
            "record":{"plan":{"id":"restore-case"}}, "bindings":bindings,
            "runtime":runtime,"images":evidence,
        }))
        .unwrap();
        assert_eq!(result["status"], "external_copy_completed");
        assert_eq!(result["runtime"], runtime);
        let reply = finish(&started, Ok(result.clone()), evidence).unwrap();
        assert_eq!(reply, result);
        let path = fixture.work.join(format!("restore-{side}.json"));
        let terminal = native::json::read(&path).unwrap();
        assert_eq!(terminal["status"], "completed");
        assert_eq!(terminal["result"], result);
        assert!(terminal.get("resource_images").is_none());
        for (key, value) in bindings.as_object().unwrap() {
            assert_eq!(&terminal[key], value);
            assert_eq!(&terminal["result"][key], value);
        }
        for phase in ["before", "after"] {
            let image = fixture
                .work
                .join(format!("restore-{side}-{phase}/image.json"));
            assert_eq!(
                terminal["result"]["resource_images"][phase],
                common::binding(&image).unwrap()
            );
        }
        assert_eq!(writes.get(), 1);
        assert!(!fixture.work.join("restore.json").exists());
        let bytes = fs::read(&path).unwrap();
        drop(lock);
        assert!(receipts::start(&fixture.args).is_err());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn mismatched_before_image_prevents_writes_and_finishes_with_original_evidence() {
    let fixture = Fixture::new("base");
    let lock = Lock::claim(&fixture.work).unwrap();
    let started = receipts::start(&fixture.args).unwrap();
    let mut evidence = Map::new();
    let result = capture_around_write(
        |phase, evidence| {
            lock.check()?;
            receipts::check_started(&started)?;
            assert!(phase == Phase::Before);
            fixture.image(phase, evidence, false)
        },
        || panic!("前像不匹配时不得发送写请求"),
        &mut evidence,
    );
    assert!(finish(&started, result.map(|()| Value::Null), evidence).is_err());
    let receipt = native::json::read(&fixture.work.join("restore-base.json")).unwrap();
    assert_eq!(receipt["status"], "failed");
    assert!(receipt.get("result").is_none());
    let images = receipt["resource_images"].as_object().unwrap();
    assert_eq!(
        images.keys().map(String::as_str).collect::<Vec<_>>(),
        ["before", "before-failure"]
    );
    let (_, before) =
        document::bound_file(&fixture.backend, &images["before"], "实际前像").unwrap();
    assert_eq!(before.value["matches_expected"], false);
    assert!(!fixture.work.join("restore-base-after").exists());
    assert!(!fixture.work.join("restore-base-failure-after").exists());
}
