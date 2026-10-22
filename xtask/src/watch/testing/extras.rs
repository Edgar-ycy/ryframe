use super::*;

#[test]
fn tools_parent_maintenance_failure_closes_fence_before_failure_is_consumed() {
    let fixture = Fixture::new(false);
    let watcher = fixture.watcher(40);
    let before = watcher.current_revision();
    assert!(watch::testing::replace_tools_parent_with_file(&watcher));
    let after = watcher.current_revision();
    assert_eq!(after, before, "维护失败不得伪造正常路径变更");
    // 辅助入口只等待 raw 处理，尚未读取任何逻辑事件。
    assert!(!watcher.final_revision_fence(after));
    assert!(watcher.is_superseded(after));
    let mut failure = false;
    while let Some(event) = watcher.recv_timeout(Duration::ZERO).unwrap() {
        match event {
            WatchEvent::Failed(message) => {
                assert!(!message.contains("must-not-echo-tools-parent-content"));
                assert!(message.contains("请"));
                failure = true;
            }
            WatchEvent::BackendChanged { .. } => panic!("维护失败不得发布正常变更"),
        }
    }
    assert!(failure, "维护失败必须发送明确错误");
    assert!(!watcher.final_revision_fence(after));
}

#[test]
fn allocation_and_send_remain_serialized_when_first_callback_is_paused() {
    let fixture = Fixture::new(false);
    let harness = watch::testing::Harness::new(&fixture.root, 2);
    let gate = harness.gate();
    for _ in 0..16 {
        harness.interleave_callbacks();
    }
    assert_eq!(harness.counts(), (32, 0, 2));
    assert!(!harness.fence_under_gate(2));
    drop(gate);
    harness.barrier();
    assert_eq!(harness.counts(), (32, 32, 2));
    assert!(harness.events().is_empty());
    assert!(harness.fence(2));
}

#[test]
fn two_native_generations_send_concurrently_without_false_sequence_gaps() {
    let fixture = Fixture::new(false);
    let harness = watch::testing::Harness::new(&fixture.root, 100);
    let gate = harness.gate();
    harness.concurrent_callbacks(64);
    assert_eq!(harness.counts(), (128, 0, 100));
    drop(gate);
    harness.barrier();
    assert_eq!(harness.counts(), (128, 128, 228));
    let events = harness.events();
    assert_eq!(events.len(), 128);
    for (index, event) in events.into_iter().enumerate() {
        assert!(
            matches!(event, WatchEvent::BackendChanged { revision, paths }
            if revision.value() == 101 + u64::try_from(index).unwrap()
                && paths == BTreeSet::from(["Cargo.toml".into()]))
        );
    }
    assert!(harness.fence(228));
}

#[test]
fn logical_revision_exhaustion_reports_failure_without_wrap_or_panic() {
    let fixture = Fixture::new(false);
    let harness = watch::testing::Harness::new(&fixture.root, u64::MAX - 1);
    harness.send(&["Cargo.toml"]);
    harness.send(&["Cargo.toml"]);
    harness.barrier();
    assert_eq!(harness.counts(), (2, 2, u64::MAX));
    let events = harness.events();
    assert!(
        matches!(events.as_slice(), [WatchEvent::BackendChanged { revision, .. }, WatchEvent::Failed(message)]
        if revision.value() == u64::MAX && message.contains("逻辑代次耗尽") && message.contains("重新启动"))
    );
    assert!(!harness.fence(u64::MAX));
    assert!(harness.superseded(u64::MAX));
}

#[test]
fn failed_unpublished_instance_error_does_not_poison_current_health() {
    let fixture = Fixture::new(false);
    let harness = watch::testing::Harness::new(&fixture.root, 15);
    let gate = harness.gate();
    harness.foreign_error();
    assert!(!harness.fence_under_gate(15));
    drop(gate);
    harness.barrier();
    assert_eq!(harness.counts(), (1, 1, 15));
    assert!(harness.events().is_empty());
    assert!(harness.fence(15));
}

#[test]
fn normal_workspace_events_from_retired_or_failed_preparation_are_not_discarded() {
    let fixture = Fixture::new(false);
    let harness = watch::testing::Harness::new(&fixture.root, 5);
    harness.foreign_paths(&["crates/example/src/lib.rs"]);
    harness.barrier();
    assert_eq!(harness.counts(), (1, 1, 6));
    assert!(
        matches!(harness.events().as_slice(), [WatchEvent::BackendChanged { paths, .. }] if paths.contains("crates/example/src/lib.rs"))
    );
    assert!(harness.fence(6));
}

fn wait_for_fence(watcher: &SourceWatcher, revision: SourceRevision) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        watch::testing::flush(watcher);
        if watcher.final_revision_fence(revision) {
            return;
        }
        assert!(Instant::now() < deadline, "raw 队列未在截止时间内稳定处理");
        let events = drain(watcher);
        assert!(events.is_empty(), "隔离的新实例失败不得产生当前实例变更");
    }
}

#[test]
fn cross_generation_nested_config_switch_keeps_new_parent_and_recursive_watch_alive() {
    let fixture = Fixture::new(false);
    let nested = fixture.inputs.config_dir.join("next");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("app.dev.toml"), "[app]\n").unwrap();
    let mut watcher = fixture.watcher(10);
    let next = WatchInputRoots {
        config_dir: nested.clone(),
        locales_dir: fixture.inputs.locales_dir.clone(),
    };
    watcher.update_inputs(next.clone()).unwrap();
    assert!(drain(&watcher).contains("config/__root__"));
    let registrations = watch::testing::registration_paths(&watcher);
    assert!(registrations.contains(&(fixture.inputs.config_dir.canonicalize().unwrap(), false)));
    assert!(registrations.contains(&(nested.canonicalize().unwrap(), true)));
    fs::write(fixture.inputs.config_dir.join("old.toml"), "value='old'\n").unwrap();
    fs::write(nested.join("app.dev.toml"), "[app]\nport=8082\n").unwrap();
    let (_, mut paths) = wait_for_paths(&watcher, "config/app.dev.toml");
    paths.extend(drain(&watcher));
    assert!(!paths.contains("config/old.toml"));
    watcher.update_inputs(next).unwrap();
    drain(&watcher);
    fs::write(nested.join("after-refresh.toml"), "value='new'\n").unwrap();
    wait_for(&watcher, "config/after-refresh.toml");
}

#[test]
fn preparation_failure_after_actual_registration_keeps_old_mapping_health_and_native_watch() {
    let fixture = Fixture::new(false);
    let base = fixture.directory.path().join("candidate");
    let next = WatchInputRoots {
        config_dir: base.join("config"),
        locales_dir: base.join("locales"),
    };
    fs::create_dir_all(&next.config_dir).unwrap();
    fs::create_dir_all(&next.locales_dir).unwrap();
    let mut watcher = fixture.watcher(90);
    drain(&watcher);
    let before = watcher.current_revision();
    let error = watch::testing::failed_replacement(&mut watcher, next)
        .unwrap_err()
        .to_string();
    assert!(error.contains("登记失败"));
    wait_for_fence(&watcher, before);
    fs::write(
        fixture.inputs.config_dir.join("app.dev.toml"),
        "[app]\nport=8083\n",
    )
    .unwrap();
    assert!(wait_for(&watcher, "config/app.dev.toml") > before);
}

#[test]
fn failed_current_instance_cannot_be_replaced_with_a_healthy_flow_and_reuse_plan() {
    let fixture = Fixture::new(false);
    let mut watcher = fixture.watcher(8);
    let before = watcher.current_revision();
    watch::testing::fail_current(&watcher);
    assert!(!watcher.final_revision_fence(before));
    let error = watcher
        .update_inputs(fixture.inputs)
        .unwrap_err()
        .to_string();
    assert!(error.contains("已报告错误"));
    assert!(watcher.is_superseded(before));
    assert!(!watcher.final_revision_fence(before));
}

#[test]
fn native_plan_omits_recursive_descendants_and_parents_already_covered_recursively() {
    let mut fixture = Fixture::new(false);
    fixture.inputs.config_dir = fixture.root.join("crates/settings");
    fs::create_dir_all(&fixture.inputs.config_dir).unwrap();
    let watcher = fixture.watcher(0);
    let registrations = watch::testing::registration_paths(&watcher);
    assert!(registrations.contains(&(fixture.root.join("crates").canonicalize().unwrap(), true)));
    assert!(
        !registrations
            .iter()
            .any(|(path, _)| path == &fixture.inputs.config_dir.canonicalize().unwrap())
    );
    assert_eq!(
        registrations
            .iter()
            .map(|(path, _)| path)
            .collect::<BTreeSet<_>>()
            .len(),
        registrations.len()
    );
}

#[test]
fn generic_constructor_observes_local_tools_when_parent_is_created_later() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let watcher = SourceWatcher::new(&root).unwrap();
    fs::create_dir(root.join(".local-tests")).unwrap();
    fs::write(root.join(".local-tests/tools.toml"), "[development]\n").unwrap();
    wait_for(&watcher, "config/tools.local.toml");
}

#[test]
fn fully_registered_new_instance_error_before_publish_keeps_previous_health_and_registration() {
    let fixture = Fixture::new(false);
    let mut watcher = fixture.watcher(100);
    let before = watcher.current_revision();
    let registrations = watch::testing::registration_paths(&watcher);
    let error = watch::testing::new_fails_before_publish(&mut watcher, fixture.inputs.clone())
        .unwrap_err()
        .to_string();
    assert!(error.contains("新监听实例"));
    wait_for_fence(&watcher, before);
    assert_eq!(watch::testing::registration_paths(&watcher), registrations);
    fs::write(
        fixture.inputs.locales_dir.join("still-live.toml"),
        "value='current'\n",
    )
    .unwrap();
    wait_for(&watcher, "locales/still-live.toml");
}

#[test]
fn old_instance_failure_during_new_preparation_rejects_publish_and_fails_closed() {
    let fixture = Fixture::new(false);
    let mut watcher = fixture.watcher(12);
    let before = watcher.current_revision();
    let registrations = watch::testing::registration_paths(&watcher);
    let error = watch::testing::old_fails_before_publish(&mut watcher, fixture.inputs)
        .unwrap_err()
        .to_string();
    assert!(error.contains("旧监听实例"));
    assert!(error.contains("已报告错误"));
    assert_eq!(watch::testing::registration_paths(&watcher), registrations);
    assert!(!watcher.final_revision_fence(before));
    assert!(watcher.is_superseded(before));
}
