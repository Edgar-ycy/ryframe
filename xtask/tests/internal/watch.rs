use std::{fs, time::Duration};

use super::watch::{SourceWatcher, WatchEvent, is_backend_watch_path};

#[test]
fn watches_backend_inputs_but_ignores_build_outputs_and_docs() {
    for path in [
        "Cargo.toml",
        "crates/ryframe/src/main.rs",
        "crates/ryframe/Cargo.toml",
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
    let Some(WatchEvent::BackendChanged(path)) = event else {
        panic!("应收到后端源码变更");
    };
    let event = watcher
        .drain_changes(path, Duration::from_millis(100))
        .unwrap();

    assert!(matches!(
        event,
        WatchEvent::BackendChanged(path) if path == "crates/demo/src/lib.rs"
    ));
    drop(watcher);
    fs::remove_dir_all(root).unwrap();
}
