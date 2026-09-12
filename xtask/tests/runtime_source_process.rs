//! Runtime/Source 公开入口只通过固定私有协议启动 Python，不把证据路径放入 argv。

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    registration: PathBuf,
    target: PathBuf,
    source_receipt: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let directory = root.join(".local-tests").join(format!(
            "xtask-runtime-source-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self {
            registration: write(&directory, "运行 登记.json"),
            target: write(&directory, "目标 计划.json"),
            source_receipt: write(&directory, "来源 比较.json"),
            directory,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn write(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, b"{}\n").unwrap();
    path
}

fn invoke(arguments: &[String], protocol_key: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(arguments)
        .env(
            "RYFRAME_RESTORE_RUNTIME_PROTOCOL",
            "untrusted inherited runtime value",
        )
        .env(
            "RYFRAME_RESTORE_SOURCE_PROTOCOL",
            "untrusted inherited source value",
        )
        .env(protocol_key, "untrusted inherited current value")
        .env("PYTHONUTF8", "1")
        .output()
        .unwrap()
}

fn assert_private_dispatch(result: Output, script: &str, hidden_paths: &[&Path]) {
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stdout.contains(script), "{stdout}");
    for path in hidden_paths {
        assert!(!stdout.contains(path.to_str().unwrap()), "{stdout}");
    }
    assert!(!stderr.contains("protocol_error"), "{stderr}");
}

#[test]
fn runtime_status_replaces_inherited_protocol_and_hides_evidence_paths_from_argv() {
    let fixture = Fixture::new();
    let arguments = vec![
        "check".into(),
        "recovery".into(),
        "runtime".into(),
        "status".into(),
        "--runtime-registration".into(),
        fixture.registration.to_string_lossy().into_owned(),
        "--target-plan".into(),
        fixture.target.to_string_lossy().into_owned(),
    ];
    let result = invoke(&arguments, "RYFRAME_RESTORE_RUNTIME_PROTOCOL");
    assert_private_dispatch(
        result,
        "scripts/restore_runtime.py",
        &[&fixture.registration, &fixture.target],
    );
}

#[test]
fn source_comparison_verify_replaces_protocol_and_hides_receipt_path_from_argv() {
    let fixture = Fixture::new();
    let arguments = vec![
        "check".into(),
        "recovery".into(),
        "source".into(),
        "comparison-verify".into(),
        "--receipt".into(),
        fixture.source_receipt.to_string_lossy().into_owned(),
    ];
    let result = invoke(&arguments, "RYFRAME_RESTORE_SOURCE_PROTOCOL");
    assert_private_dispatch(
        result,
        "scripts/restore_source.py",
        &[&fixture.source_receipt],
    );
}
