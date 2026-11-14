use super::*;
use crate::recovery::clone::native::lifecycle::{lock::RunLock, state};
use std::{cell::Cell, path::Path};

fn setup() -> (PathBuf, tempfile::TempDir, Receipts) {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(&local).unwrap();
    let root = directory.path();
    let run = root.join("run");
    fs::create_dir(&run).unwrap();
    document::write_new(&run.join("manifest.json"), &json!({"fixture":true})).unwrap();
    state::initialize(&run).unwrap();
    let lock = RunLock::claim(&run).unwrap();
    let (number, record) = state::begin(&run, "target-cache-markers", "run", &json!({})).unwrap();
    state::bind_controller(&run, number, &lock.owner, &record).unwrap();
    let controller = state::controller_record(&run, number, &record).unwrap().0;
    lock.release().unwrap();
    let evidence = root.join("evidence");
    fs::create_dir(&evidence).unwrap();
    let receipts = Receipts {
        backend: backend.clone(),
        directory: evidence.clone(),
        intent: evidence.join("intent.json"),
        attempt: number,
        binding: json!({"registered":1}),
        head: json!({"sequence":235}),
        after: json!({"keys":["owner"],"owner":"owner","sentinel":"sentinel"}),
        controller,
        run,
        protected: Inputs::directory(&evidence).unwrap(),
    };
    (backend, directory, receipts)
}

fn observation(backend: &Path, value: &Value) -> Value {
    common::read_bound(backend, &value["observation"], "观察收据").unwrap()
}

#[test]
fn mixed_and_already_complete_images_issue_no_write_intent() {
    for (classification, observed, passed) in [
        (
            "mismatch",
            json!({"keys":["other"],"owner":null,"sentinel":null}),
            false,
        ),
        (
            "after",
            json!({"keys":["owner"],"owner":"owner","sentinel":"sentinel"}),
            true,
        ),
    ] {
        let (backend, _directory, recovery) = setup();
        let value = recovery
            .apply(&observed, &|| Ok(()), |_| panic!("不应发出 Redis 写入"))
            .unwrap();
        assert_eq!(value["classification"], classification);
        assert_eq!(observation(&backend, &value)["confirmed"], passed);
        assert_eq!(observation(&backend, &value)["remote_writes"], 0);
        assert!(!recovery.intent.exists());
    }
}

#[test]
fn lost_response_keeps_durable_intent_and_confirms_only_by_read_only_reconciliation() {
    let (backend, _directory, mut recovery) = setup();
    let calls = Cell::new(0);
    let value = recovery
        .apply(&empty_image(), &|| Ok(()), |begin| {
            assert!(recovery.intent.is_file());
            begin()?;
            calls.set(calls.get() + 1);
            assert!(begin().is_err());
            Err("测试模拟 Redis 已写但响应超时".into())
        })
        .unwrap();
    assert_eq!(value["classification"], "unknown");
    assert_eq!(value["marker_write_attempts"], 1);
    assert!(observation(&backend, &value)["remote_writes"].is_null());
    assert!(recovery.require_unattempted().is_err());
    state::finish(
        &recovery.run,
        recovery.attempt,
        Some(&value),
        Some("NeedsReconciliation"),
    )
    .unwrap();
    recovery.attempt += 1;
    let outcome = recovery
        .reconcile(
            |head| {
                assert_eq!(head, &json!({"sequence":235}));
                Ok(())
            },
            || Ok(recovery.after.clone()),
            &|| Ok(()),
        )
        .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(outcome["status"], "target_cache_markers_confirmed");
    assert_eq!(observation(&backend, &outcome)["remote_writes"], 0);
}

#[test]
fn a_checkpoint_failure_before_dispatch_records_zero_attempts_and_still_blocks_replay() {
    let (backend, _directory, recovery) = setup();
    let checkpoints = Cell::new(0);
    let value = recovery
        .apply(
            &empty_image(),
            &|| {
                let current = checkpoints.get();
                checkpoints.set(current + 1);
                if current == 0 {
                    Ok(())
                } else {
                    Err("测试模拟源证据漂移".into())
                }
            },
            |begin| {
                begin()?;
                panic!("前置守卫失败不应发出 Redis 写入")
            },
        )
        .unwrap();
    assert_eq!(value["classification"], "blocked_before_write");
    assert_eq!(value["marker_write_attempts"], 0);
    assert_eq!(observation(&backend, &value)["remote_writes"], 0);
    assert!(recovery.require_unattempted().is_err());
}

#[test]
fn reconciliation_keeps_an_empty_image_unconfirmed_and_refuses_prefix_drift() {
    let (backend, _directory, mut recovery) = setup();
    let value = recovery
        .apply(&empty_image(), &|| Ok(()), |begin| {
            begin()?;
            Err("测试模拟 Redis 请求未获回应".into())
        })
        .unwrap();
    state::finish(
        &recovery.run,
        recovery.attempt,
        Some(&value),
        Some("NeedsReconciliation"),
    )
    .unwrap();
    recovery.attempt += 1;
    assert!(
        recovery
            .reconcile(
                |_| Err("测试模拟复制前缀变化".into()),
                || panic!("复制前缀变化不应建立 Redis 连接"),
                &|| Ok(()),
            )
            .is_err()
    );
    let value = recovery
        .reconcile(|_| Ok(()), || Ok(empty_image()), &|| Ok(()))
        .unwrap();
    assert_eq!(value["classification"], "before");
    assert_eq!(observation(&backend, &value)["confirmed"], false);
    assert!(recovery.require_unattempted().is_err());
}
