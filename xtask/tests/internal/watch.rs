use std::{fs, path::Path, time::Duration};

use super::{
    source_edit::SourceEdit,
    watch::{SourceWatcher, WatchEvent, is_backend_watch_path, relevant_paths},
};

#[test]
fn watches_backend_inputs_but_ignores_build_outputs_and_docs() {
    for path in [
        "Cargo.toml",
        ".cargo/config.toml",
        "crates/ryframe/src/main.rs",
        "crates/ryframe/Cargo.toml",
        "xtask/src/dev.rs",
        "config/app.dev.toml",
        "catalog/resources/post.toml",
        "locales/zh-CN.toml",
    ] {
        assert!(is_backend_watch_path(path), "应监听 {path}");
    }
    for path in [
        "target/debug/ryframe.exe",
        ".git/index",
        ".local-tests/result.json",
        "docs/development.md",
        "README.md",
    ] {
        assert!(!is_backend_watch_path(path), "不应监听 {path}");
    }
}

#[test]
fn one_notify_event_preserves_every_relevant_path() {
    let root = Path::new("D:/workspace");
    let paths = relevant_paths(
        root,
        &[
            root.join("crates/ryframe/src/main.rs"),
            root.join("target/debug/ryframe.exe"),
            root.join("xtask/src/dev.rs"),
            root.join("crates/ryframe/src/main.rs"),
        ],
    );

    assert_eq!(
        paths.into_iter().collect::<Vec<_>>(),
        ["crates/ryframe/src/main.rs", "xtask/src/dev.rs"]
    );
}

#[test]
fn source_watcher_reports_backend_file_change_and_stops_cleanly() {
    let root = std::env::temp_dir().join(format!("ryframe-xtask-watch-{}", std::process::id()));
    let source = root.join("crates/demo/src/lib.rs");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "pub fn before() {}\n").unwrap();
    let watcher = SourceWatcher::new(&root).unwrap();
    std::thread::sleep(Duration::from_millis(100));

    fs::write(&source, "pub fn after() {}\n").unwrap();
    let event = watcher.recv_timeout(Duration::from_secs(3)).unwrap();
    let Some(WatchEvent::BackendChanged { revision, paths }) = event else {
        panic!("应收到后端源码变更");
    };
    let batch = watcher
        .drain_changes(revision, paths, Duration::from_millis(100))
        .unwrap();

    assert_eq!(
        batch.paths.into_iter().collect::<Vec<_>>(),
        ["crates/demo/src/lib.rs"]
    );
    assert!(batch.revision.value() >= 1);
    assert_eq!(watcher.current_revision(), batch.revision);
    drop(watcher);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn atomic_save_edit_reaches_the_real_watcher_as_one_source_path() {
    let root =
        std::env::temp_dir().join(format!("ryframe-xtask-watch-atomic-{}", std::process::id()));
    let source = root.join("config/app.dev.toml");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let original = b"[app]\nport = 8080\n";
    fs::write(&source, original).unwrap();
    let watcher = SourceWatcher::new(&root).unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let (mut edit, _) = SourceEdit::apply(&source, "watcher", "#").unwrap();
    let Some(WatchEvent::BackendChanged { revision, paths }) =
        watcher.recv_timeout(Duration::from_secs(3)).unwrap()
    else {
        panic!("原子保存应触发后端 watcher");
    };
    let batch = watcher
        .drain_changes(revision, paths, Duration::from_millis(100))
        .unwrap();

    assert_eq!(batch.paths, ["config/app.dev.toml".to_owned()].into());
    edit.restore().unwrap();
    assert_eq!(fs::read(&source).unwrap(), original);
    drop(watcher);
    fs::remove_dir_all(root).unwrap();
}
