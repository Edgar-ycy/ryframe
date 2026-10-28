//! Runtime 保留受管协议；来源验证与比较的失败请求由原生入口只读拒绝。

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    capture: PathBuf,
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
        let capture = directory.join("Python 环境.json");
        fs::write(
            directory.join("sitecustomize.py"),
            r#"from pathlib import Path
Path(__file__).with_name("Python 环境.json").write_text("ambient-hook-executed", encoding="utf-8")
"#,
        )
        .unwrap();
        Self {
            capture,
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

fn invoke_with_conflicting_python_environment(arguments: &[String], fixture: &Fixture) -> Output {
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
        .env("pythonUtf8", "0")
        .env("PythonIoEncoding", "ascii:strict")
        .env("PYTHONPATH", &fixture.directory)
        .env("RYFRAME_RUNTIME_TEST_CAPTURE", &fixture.capture)
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
fn runtime_status_uses_managed_file_and_rejects_ambient_python_hooks() {
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
    let result = invoke_with_conflicting_python_environment(&arguments, &fixture);
    let stdout = String::from_utf8(result.stdout.clone()).unwrap();
    assert!(
        stdout.contains("-X utf8 -E -s -B tools/python/restore_runtime.py"),
        "{stdout}"
    );
    assert_private_dispatch(
        result,
        "tools/python/restore_runtime.py",
        &[&fixture.registration, &fixture.target],
    );
    assert!(!fixture.capture.exists());
    assert!(!fixture.directory.join("__pycache__").exists());
}

#[test]
fn source_comparison_rejects_invalid_receipt_without_ambient_hooks_or_file_changes() {
    let fixture = Fixture::new();
    let arguments = vec![
        "check".into(),
        "recovery".into(),
        "source".into(),
        "comparison-verify".into(),
        "--receipt".into(),
        fixture.source_receipt.to_string_lossy().into_owned(),
    ];
    let before = fs::read_dir(&fixture.directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (path.clone(), fs::read(path).unwrap())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let result = invoke_with_conflicting_python_environment(&arguments, &fixture);
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let after = fs::read_dir(&fixture.directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (path.clone(), fs::read(path).unwrap())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(before, after);
    assert!(!fixture.capture.exists());
    assert!(!fixture.directory.join("__pycache__").exists());
}

#[test]
fn source_verify_and_recover_reject_unregistered_start_without_python_or_new_evidence() {
    let fixture = Fixture::new();
    let verification = fixture.directory.join("g0001/verification");
    fs::create_dir_all(&verification).unwrap();
    let input = fs::read(&fixture.registration).unwrap();
    let before = fs::read_dir(&fixture.directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<std::collections::BTreeSet<_>>();
    for operation in ["verify", "verify-recover"] {
        let arguments = vec![
            "check".into(),
            "recovery".into(),
            "source".into(),
            operation.into(),
            "--source-generation".into(),
            fixture.registration.to_string_lossy().into_owned(),
            "--output".into(),
            verification
                .join("source-runtime.json")
                .to_string_lossy()
                .into_owned(),
            "--write".into(),
        ];
        let result = invoke_with_conflicting_python_environment(&arguments, &fixture);
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(
            stderr.contains("来源 START 必须属于原账本 results"),
            "{stderr}"
        );
        assert!(!stderr.contains("Traceback"), "{stderr}");
        assert_eq!(fs::read(&fixture.registration).unwrap(), input);
        assert!(fs::read_dir(&verification).unwrap().next().is_none());
        assert!(!fixture.directory.join("run.lock").exists());
        assert!(!fixture.capture.exists());
        assert!(!fixture.directory.join("__pycache__").exists());
    }
    let after = fs::read_dir(&fixture.directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(before, after);
}

#[test]
fn generation_control_rejects_missing_native_manifest_without_python_or_stage_publication() {
    let fixture = Fixture::new();
    let before = fs::read_dir(&fixture.directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (path.clone(), fs::read(path).unwrap())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    for operation in ["source-generation-stop", "source-generation-recover"] {
        let arguments = vec![
            "check".into(),
            "recovery".into(),
            "clone".into(),
            "seed-runtime".into(),
            "--operation".into(),
            operation.into(),
            "--run-dir".into(),
            fixture.directory.to_string_lossy().into_owned(),
            "--request".into(),
            fixture.source_receipt.to_string_lossy().into_owned(),
            "--write".into(),
        ];
        let result = invoke_with_conflicting_python_environment(&arguments, &fixture);
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(stderr.contains("复制运行清单无效"), "{stderr}");
        assert!(!stderr.contains("Traceback"), "{stderr}");
        assert!(!fixture.capture.exists());
    }
    let after = fs::read_dir(&fixture.directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (path.clone(), fs::read(path).unwrap())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(before, after);
}
