use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use tempfile::{TempDir, tempdir};

use crate::watch::{
    SourceRevision, SourceRevisionTracker, SourceWatcher, WatchEvent, WatchInputRoots,
};
use crate::{source_edit, watch};

struct Fixture {
    directory: TempDir,
    root: PathBuf,
    inputs: WatchInputRoots,
}

impl Fixture {
    fn new(ignored: bool) -> Self {
        let directory = tempdir().unwrap();
        let root = directory.path().join("workspace");
        fs::create_dir_all(root.join(".local-tests")).unwrap();
        let base = if ignored {
            root.join(".local-tests/inputs")
        } else {
            directory.path().join("external")
        };
        let inputs = WatchInputRoots {
            config_dir: base.join("configuration"),
            locales_dir: base.join("translations"),
        };
        fs::create_dir_all(&inputs.config_dir).unwrap();
        fs::create_dir_all(&inputs.locales_dir).unwrap();
        fs::write(inputs.config_dir.join("app.dev.toml"), "[app]\nport=8080\n").unwrap();
        fs::write(inputs.locales_dir.join("zh-CN.toml"), "hello='原值'\n").unwrap();
        Self {
            directory,
            root,
            inputs,
        }
    }

    fn watcher(&self, seed: u64) -> SourceWatcher {
        SourceWatcher::new_with_inputs(
            &self.root,
            self.inputs.clone(),
            SourceRevision::from_value(seed),
        )
        .unwrap()
    }
}

fn wait_for(watcher: &SourceWatcher, expected: &str) -> SourceRevision {
    wait_for_paths(watcher, expected).0
}

fn wait_for_paths(watcher: &SourceWatcher, expected: &str) -> (SourceRevision, BTreeSet<String>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut observed = BTreeSet::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "未收到指定逻辑路径：{expected}");
        match watcher.recv_timeout(remaining).unwrap() {
            Some(WatchEvent::BackendChanged { revision, paths }) => {
                observed.extend(paths);
                if observed.contains(expected) {
                    return (revision, observed);
                }
            }
            Some(WatchEvent::Failed(error)) => panic!("监听失败：{error}"),
            None => panic!("未收到指定逻辑路径：{expected}"),
        }
    }
}

fn drain(watcher: &SourceWatcher) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    while let Some(event) = watcher.recv_timeout(Duration::from_millis(100)).unwrap() {
        match event {
            WatchEvent::BackendChanged { paths: next, .. } => paths.extend(next),
            WatchEvent::Failed(error) => panic!("监听失败：{error}"),
        }
    }
    paths
}

#[test]
fn external_and_ignored_inputs_generate_selected_logical_paths() {
    for ignored in [false, true] {
        let fixture = Fixture::new(ignored);
        let watcher = fixture.watcher(10);
        fs::write(
            fixture.inputs.config_dir.join("app.dev.toml"),
            "[app]\nport=8081\n",
        )
        .unwrap();
        assert!(wait_for(&watcher, "config/app.dev.toml").value() > 10);
        fs::write(
            fixture.inputs.locales_dir.join("zh-CN.toml"),
            "hello='新值'\n",
        )
        .unwrap();
        wait_for(&watcher, "locales/zh-CN.toml");
    }
}

#[test]
fn atomic_save_in_selected_input_preserves_logical_path_and_source_restore() {
    let fixture = Fixture::new(false);
    let watcher = fixture.watcher(0);
    let source = fixture.inputs.config_dir.join("app.dev.toml");
    let before = fs::read(&source).unwrap();
    let (mut edit, started) = source_edit::SourceEdit::apply(&source, "selected", "#").unwrap();
    assert!(started.monotonic <= Instant::now());
    assert!(
        started
            .wall_clock
            .duration_since(std::time::UNIX_EPOCH)
            .is_ok()
    );
    wait_for(&watcher, "config/app.dev.toml");
    edit.restore().unwrap();
    assert_eq!(fs::read(source).unwrap(), before);
}

#[test]
fn replaced_input_roots_report_root_change_and_refresh_receives_new_file_events() {
    for config in [true, false] {
        let fixture = Fixture::new(false);
        let mut watcher = fixture.watcher(4);
        let (root, name, logical_root, logical_file) = if config {
            (
                &fixture.inputs.config_dir,
                "app.dev.toml",
                "config/__root__",
                "config/app.dev.toml",
            )
        } else {
            (
                &fixture.inputs.locales_dir,
                "zh-CN.toml",
                "locales/__root__",
                "locales/zh-CN.toml",
            )
        };
        let old = root.with_file_name(if config {
            "previous-config"
        } else {
            "previous-locales"
        });
        fs::rename(root, &old).unwrap();
        fs::create_dir(root).unwrap();
        wait_for(&watcher, logical_root);
        watcher.update_inputs(fixture.inputs.clone()).unwrap();
        drain(&watcher);
        let before = watcher.current_revision();
        watcher.update_inputs(fixture.inputs.clone()).unwrap();
        assert_eq!(watcher.current_revision(), before);
        fs::write(root.join(name), "value='replacement'\n").unwrap();
        assert!(wait_for(&watcher, logical_file) > before);
    }
}

#[test]
fn input_switch_invalidates_old_fence_and_stops_mapping_old_external_paths() {
    let fixture = Fixture::new(false);
    let mut watcher = fixture.watcher(70);
    let old_revision = watcher.current_revision();
    assert!(watcher.final_revision_fence(old_revision));
    let base = fixture.directory.path().join("next");
    let next = WatchInputRoots {
        config_dir: base.join("config"),
        locales_dir: base.join("locales"),
    };
    fs::create_dir_all(&next.config_dir).unwrap();
    fs::create_dir_all(&next.locales_dir).unwrap();
    watcher.update_inputs(next.clone()).unwrap();
    assert!(watcher.current_revision() > old_revision);
    assert!(watcher.is_superseded(old_revision));
    assert!(!watcher.final_revision_fence(old_revision));
    let changed = drain(&watcher);
    assert!(changed.contains("config/__root__"));
    assert!(changed.contains("locales/__root__"));
    fs::write(fixture.inputs.config_dir.join("old.toml"), "value='old'\n").unwrap();
    fs::write(fixture.inputs.locales_dir.join("old.toml"), "value='old'\n").unwrap();
    fs::write(next.config_dir.join("marker.toml"), "value='next'\n").unwrap();
    let (_, mut remaining) = wait_for_paths(&watcher, "config/marker.toml");
    remaining.extend(drain(&watcher));
    assert!(!remaining.contains("config/old.toml"));
    assert!(!remaining.contains("locales/old.toml"));
}

#[test]
fn local_tool_file_is_relevant_but_other_local_outputs_remain_ignored() {
    let fixture = Fixture::new(false);
    fs::create_dir(fixture.root.join("config")).unwrap();
    let watcher = fixture.watcher(0);
    fs::write(
        fixture.root.join(".local-tests/result.json"),
        "{\"secret\":\"must-not-echo\"}",
    )
    .unwrap();
    fs::write(
        fixture.root.join(".local-tests/tools.toml"),
        "[development]\n",
    )
    .unwrap();
    wait_for(&watcher, "config/tools.local.toml");
    assert!(
        !drain(&watcher)
            .iter()
            .any(|path| path.contains("result.json"))
    );
    fs::write(fixture.root.join("config/tools.toml"), "[development]\n").unwrap();
    wait_for(&watcher, "config/tools.toml");
}

#[test]
fn selected_workspace_inputs_take_priority_over_natural_source_paths() {
    let mut fixture = Fixture::new(false);
    fixture.inputs = WatchInputRoots {
        config_dir: fixture.root.join("crates/settings"),
        locales_dir: fixture.root.join("vendor/translations"),
    };
    fs::create_dir_all(&fixture.inputs.config_dir).unwrap();
    fs::create_dir_all(&fixture.inputs.locales_dir).unwrap();
    let watcher = fixture.watcher(0);
    fs::write(fixture.inputs.config_dir.join("app.dev.toml"), "[app]\n").unwrap();
    let (_, paths) = wait_for_paths(&watcher, "config/app.dev.toml");
    assert!(!paths.contains("crates/settings/app.dev.toml"));
    fs::write(
        fixture.inputs.locales_dir.join("zh-CN.toml"),
        "hello='value'\n",
    )
    .unwrap();
    let (_, paths) = wait_for_paths(&watcher, "locales/zh-CN.toml");
    assert!(!paths.contains("vendor/translations/zh-CN.toml"));
}

#[test]
fn seed_and_existing_source_tracker_api_never_reset_revision() {
    let fixture = Fixture::new(false);
    let mut watcher = fixture.watcher(1234);
    assert_eq!(watcher.current_revision().value(), 1234);
    watcher.update_inputs(fixture.inputs.clone()).unwrap();
    assert_eq!(watcher.current_revision().value(), 1234);
    fs::write(
        fixture.inputs.config_dir.join("app.dev.toml"),
        "[app]\nport=8082\n",
    )
    .unwrap();
    let next = wait_for(&watcher, "config/app.dev.toml");
    assert!(next.value() > 1234);
    let tracker = SourceRevisionTracker::default();
    let (revision, paths) = tracker
        .record_paths(
            Path::new("workspace"),
            &[PathBuf::from("workspace/Cargo.toml")],
        )
        .unwrap();
    assert_eq!(revision.value(), 1);
    assert!(paths.contains("Cargo.toml"));
    assert!(tracker.final_revision_fence(revision));
}

#[test]
fn explicit_inputs_reject_overlap_missing_and_non_directory_paths_without_contents() {
    let fixture = Fixture::new(false);
    let nested = fixture.inputs.config_dir.join("nested");
    fs::create_dir(&nested).unwrap();
    for inputs in [
        WatchInputRoots {
            config_dir: fixture.inputs.config_dir.clone(),
            locales_dir: fixture.inputs.config_dir.clone(),
        },
        WatchInputRoots {
            config_dir: fixture.inputs.config_dir.clone(),
            locales_dir: nested.clone(),
        },
        WatchInputRoots {
            config_dir: nested,
            locales_dir: fixture.inputs.config_dir.clone(),
        },
        WatchInputRoots {
            config_dir: fixture.directory.path().join("missing"),
            locales_dir: fixture.inputs.locales_dir.clone(),
        },
    ] {
        let message =
            SourceWatcher::new_with_inputs(&fixture.root, inputs, SourceRevision::from_value(0))
                .err()
                .expect("非法输入必须失败")
                .to_string();
        assert!(message.contains("请"));
        assert!(!message.contains("must-not-echo"));
    }
    let file = fixture.directory.path().join("file");
    fs::write(&file, "must-not-echo").unwrap();
    let message = SourceWatcher::new_with_inputs(
        &fixture.root,
        WatchInputRoots {
            config_dir: file.clone(),
            locales_dir: fixture.inputs.locales_dir.clone(),
        },
        SourceRevision::from_value(0),
    )
    .err()
    .expect("文件不能作为目录")
    .to_string();
    assert!(message.contains("config 输入"));
    assert!(message.contains(file.to_str().unwrap()));
    assert!(message.contains("请"));
    assert!(!message.contains("must-not-echo"));
}

#[cfg(any(unix, windows))]
#[test]
fn linked_input_and_linked_ancestor_are_rejected() {
    let fixture = Fixture::new(false);
    let alias = fixture.directory.path().join("alias");
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(&fixture.inputs.config_dir, &alias);
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_dir(&fixture.inputs.config_dir, &alias);
    match created {
        Ok(()) => {}
        #[cfg(windows)]
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
        Err(error) => panic!("无法创建测试链接：{error}"),
    }
    fs::create_dir(fixture.inputs.config_dir.join("child")).unwrap();
    for config_dir in [
        alias.clone(),
        alias.join("child"),
        alias.join("../configuration"),
    ] {
        let error = SourceWatcher::new_with_inputs(
            &fixture.root,
            WatchInputRoots {
                config_dir,
                locales_dir: fixture.inputs.locales_dir.clone(),
            },
            SourceRevision::from_value(0),
        )
        .err()
        .expect("链接路径必须拒绝")
        .to_string();
        assert!(error.contains("链接"));
        assert!(error.contains("请"));
    }
}

#[test]
fn queued_ignored_raw_blocks_fence_until_applied_without_changing_logical_revision() {
    let fixture = Fixture::new(false);
    let harness = watch::testing::Harness::new(&fixture.root, 80);
    let gate = harness.gate();
    harness.send(&[".local-tests/result.json"]);
    assert_eq!(harness.counts(), (1, 0, 80));
    assert!(!harness.fence_under_gate(80));
    assert!(harness.superseded(80));
    drop(gate);
    harness.barrier();
    assert_eq!(harness.counts(), (1, 1, 80));
    assert!(harness.fence(80));
    assert!(!harness.superseded(80));
    assert!(harness.events().is_empty());
}

#[test]
fn one_raw_batch_is_applied_contiguously_and_preserves_every_relevant_path() {
    let fixture = Fixture::new(false);
    let harness = watch::testing::Harness::new(&fixture.root, 20);
    let gate = harness.gate();
    harness.send(&[
        "config/app.dev.toml",
        "crates/example/src/lib.rs",
        "config/app.dev.toml",
        "target/output.json",
    ]);
    assert_eq!(harness.counts(), (1, 0, 20));
    assert!(!harness.fence_under_gate(20));
    drop(gate);
    harness.barrier();
    assert_eq!(harness.counts(), (1, 1, 21));
    let events = harness.events();
    assert_eq!(events.len(), 1);
    let WatchEvent::BackendChanged { revision, paths } = &events[0] else {
        panic!("应返回完整逻辑批次");
    };
    assert_eq!(revision.value(), 21);
    assert_eq!(
        paths,
        &BTreeSet::from([
            "config/app.dev.toml".into(),
            "crates/example/src/lib.rs".into()
        ])
    );
    assert!(!harness.fence(20));
    assert!(harness.fence(21));
}

#[test]
fn native_error_invalidates_fence_before_dispatch_and_before_error_is_read() {
    let fixture = Fixture::new(false);
    let harness = watch::testing::Harness::new(&fixture.root, 30);
    let gate = harness.gate();
    harness.error();
    assert_eq!(harness.counts(), (1, 0, 30));
    assert!(!harness.fence_under_gate(30));
    assert!(harness.superseded(30));
    drop(gate);
    harness.barrier();
    assert_eq!(harness.counts(), (1, 1, 30));
    assert!(!harness.fence(30));
    assert!(
        matches!(harness.events().as_slice(), [WatchEvent::Failed(message)] if message.contains("error.kind=") && message.contains("请检查"))
    );
    assert!(!harness.fence(30));
}

#[test]
fn sequence_gap_is_never_confirmed_by_taking_a_maximum() {
    let fixture = Fixture::new(false);
    let harness = watch::testing::Harness::new(&fixture.root, 3);
    harness.gap(2);
    harness.barrier();
    assert_eq!(harness.counts(), (0, 0, 3));
    assert!(!harness.fence(3));
    assert!(
        matches!(harness.events().as_slice(), [WatchEvent::Failed(message)] if message.contains("编号不连续"))
    );
    harness.send(&[".local-tests/ignored.json"]);
    harness.barrier();
    assert_eq!(harness.counts(), (1, 1, 3));
    assert!(!harness.fence(3));
}

#[test]
fn dispatcher_joins_even_if_a_native_sender_clone_outlives_it() {
    let fixture = Fixture::new(false);
    let harness = watch::testing::Harness::new(&fixture.root, 0);
    assert!(harness.close_with_late_sender());
}

#[test]
fn watcher_drop_has_joined_its_dispatcher_before_returning() {
    let fixture = Fixture::new(false);
    let watcher = fixture.watcher(0);
    let finished = watch::testing::completion(&watcher);
    assert!(!finished.load(std::sync::atomic::Ordering::SeqCst));
    drop(watcher);
    assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn raw_callback_can_finish_while_registration_gate_is_held() {
    let fixture = Fixture::new(false);
    let mut watcher = fixture.watcher(0);
    assert!(
        watch::testing::callback_during_registration_gate(&mut watcher, fixture.inputs).unwrap()
    );
}

#[path = "extras.rs"]
mod extras;
