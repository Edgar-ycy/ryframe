use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use super::dev::{
    BuildContext, BuildPlan, BuildResult, DevSession, RuntimeSecrets, StepResult, build_candidate,
    run_migration_validation,
};
use super::{
    process::ChildGroup,
    watch::{ChangeBatch, SourceWatcher},
};

#[test]
fn migration_validation_is_superseded_during_the_running_process() {
    let root = std::env::temp_dir().join(format!(
        "ryframe-xtask-migration-supersede-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("locales")).unwrap();
    let config = root.join("config/app.dev.toml");
    fs::write(&config, "[app]\nport = 8080\n").unwrap();
    let started_marker = root.join(".slow-migrate-started");
    let migrate = write_slow_migration_stub(&root, &started_marker);
    let watcher = SourceWatcher::new(&root).unwrap();
    thread::sleep(Duration::from_millis(100));
    let plan = BuildPlan::from_changes(&ChangeBatch {
        revision: watcher.current_revision(),
        paths: ["crates/ryframe/src/bin/ryframe_migrate.rs".to_owned()]
            .into_iter()
            .collect(),
    });
    let changed = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !started_marker.is_file() {
            assert!(
                Instant::now() < deadline,
                "迁移进程未在时限内启动，无法验证运行中取消"
            );
            thread::sleep(Duration::from_millis(20));
        }
        let changed_at = Instant::now();
        fs::write(config, "[app]\nport = 8081\n").unwrap();
        changed_at
    });
    let group = ChildGroup::new().unwrap();
    let shutdown = AtomicBool::new(false);
    let mut lkg_check = None;
    let started = Instant::now();
    let result = run_migration_validation(
        &group,
        &root,
        &migrate,
        &root.join("config"),
        &root.join("locales"),
        &RuntimeSecrets::default(),
        &shutdown,
        &watcher,
        &plan,
        &mut lkg_check,
    )
    .unwrap();

    let changed_at = changed.join().unwrap();
    assert_eq!(result, StepResult::Superseded);
    assert!(
        changed_at.elapsed() < Duration::from_secs(1),
        "运行中迁移未在一秒内被新 revision 取消：{:?}",
        changed_at.elapsed()
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(watcher);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn compile_build_is_superseded_after_fake_cargo_starts() {
    let root = std::env::temp_dir().join(format!(
        "ryframe-xtask-compile-supersede-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("locales")).unwrap();
    let marker = root.join(".fake-cargo-started");
    let fake_cargo = write_blocking_cargo_stub(&root);
    let watcher = SourceWatcher::new(&root).unwrap();
    let plan = BuildPlan::from_changes(&ChangeBatch {
        revision: watcher.current_revision(),
        paths: BTreeSet::from(["crates/ryframe-api/src/lib.rs".to_owned()]),
    });
    let session =
        DevSession::prepare_isolated(&root.join("state"), watcher.current_revision()).unwrap();
    let group = ChildGroup::new().unwrap();
    let shutdown = AtomicBool::new(false);
    let cargo_invocations = AtomicUsize::new(0);
    let context = BuildContext::new(&group, &root, &session, &shutdown, &watcher, &fake_cargo)
        .with_cargo_counter(&cargo_invocations);
    let build_started = Instant::now();
    let mut changed_at = None;
    let result = {
        let mut lkg_check = || {
            if changed_at.is_none() && marker.is_file() {
                fs::write(root.join("config/fake-change.toml"), "[app]\n")?;
                changed_at = Some(Instant::now());
            }
            if build_started.elapsed() > Duration::from_secs(3) {
                return Err("fake Cargo 未进入可取消的编译阶段".into());
            }
            Ok(())
        };
        build_candidate(&context, &plan, None, Some(&mut lkg_check)).unwrap()
    };
    let changed_at = changed_at.expect("测试必须记录保存时刻");
    assert!(matches!(result, BuildResult::Superseded));
    assert_eq!(cargo_invocations.load(Ordering::Relaxed), 1);
    assert!(watcher.current_revision() > plan.source_revision);
    assert!(changed_at.elapsed() < Duration::from_secs(1));
    drop(watcher);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
fn write_slow_migration_stub(root: &Path, marker: &Path) -> PathBuf {
    let path = root.join("slow-migrate.cmd");
    fs::write(
        &path,
        format!(
            "@echo off\r\n>\"{}\" echo started\r\nping -n 31 127.0.0.1 >NUL\r\n",
            marker.display()
        ),
    )
    .unwrap();
    path
}

#[cfg(windows)]
fn write_blocking_cargo_stub(root: &Path) -> PathBuf {
    let path = root.join("fake-cargo.cmd");
    fs::write(
        &path,
        "@echo off\r\n>\".fake-cargo-started\" echo started\r\n:wait\r\nping -n 2 127.0.0.1 >NUL\r\ngoto wait\r\n",
    )
    .unwrap();
    path
}

#[cfg(unix)]
fn write_slow_migration_stub(root: &Path, marker: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("slow-migrate.sh");
    fs::write(
        &path,
        format!("#!/bin/sh\n: > {}\nsleep 30\n", marker.display()),
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

#[cfg(unix)]
fn write_blocking_cargo_stub(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("fake-cargo.sh");
    fs::write(
        &path,
        "#!/bin/sh\n: > .fake-cargo-started\nwhile :; do sleep 60; done\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}
