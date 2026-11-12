//! 公开恢复控制与来源验证由原生入口拒绝无效请求并保留现场。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
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

    fn runtime_arguments(&self, operation: &str) -> Vec<String> {
        vec![
            "check".into(),
            "recovery".into(),
            "runtime".into(),
            operation.into(),
            "--runtime-registration".into(),
            self.registration.to_string_lossy().into_owned(),
            "--target-plan".into(),
            self.target.to_string_lossy().into_owned(),
        ]
    }

    fn entries(&self) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
        fs::read_dir(&self.directory)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let path = entry.path();
                let kind = entry.file_type().unwrap();
                let bytes = if kind.is_dir() {
                    None
                } else {
                    assert!(kind.is_file(), "{}", path.display());
                    Some(fs::read(&path).unwrap())
                };
                (path, bytes)
            })
            .collect()
    }

    fn assert_no_python_hook(&self) {
        assert!(!self.capture.exists());
        assert!(!self.directory.join("__pycache__").exists());
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

fn native_command(arguments: &[String], fixture: &Fixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
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
        .env("RYFRAME_RUNTIME_TEST_CAPTURE", &fixture.capture);
    command
}

fn invoke_with_conflicting_python_environment(arguments: &[String], fixture: &Fixture) -> Output {
    native_command(arguments, fixture).output().unwrap()
}

fn assert_native_registration_rejected(operation: &str, result: Output) {
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert_eq!(result.status.code(), Some(1), "{operation}: {stderr}");
    assert!(stdout.is_empty(), "{operation}: {stdout}");
    assert_eq!(
        stderr.trim_end(),
        "任务失败：恢复运行登记字段缺失或包含未知字段",
        "{operation}: {stderr}"
    );
}

#[test]
fn runtime_control_rejects_invalid_registration_without_python_or_file_changes() {
    let fixture = Fixture::new();
    let owner = fixture.directory.join("隔离 owner.json");
    let owner_bytes = b"{\"isolated_owner\":true}\n";
    fs::write(&owner, owner_bytes).unwrap();
    let before = fixture.entries();

    for operation in ["status", "stop", "recover"] {
        let mut arguments = fixture.runtime_arguments(operation);
        if operation != "status" {
            arguments.extend(["--generation".into(), "1".into(), "--write".into()]);
        }
        if operation == "recover" {
            arguments.extend(["--owner".into(), owner.to_string_lossy().into_owned()]);
        }
        let result = invoke_with_conflicting_python_environment(&arguments, &fixture);
        assert_native_registration_rejected(operation, result);
        assert_eq!(fs::read(&owner).unwrap(), owner_bytes);
        fixture.assert_no_python_hook();
        assert_eq!(fixture.entries(), before);
    }
}

#[test]
fn runtime_verify_reads_explicit_stdin_and_rejects_an_invalid_receipt_without_changes() {
    let fixture = Fixture::new();
    let backend = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let frontend = fixture.directory.join("前端 来源");
    fs::create_dir(&frontend).unwrap();
    let receipt = write(&fixture.directory, "运行 收据.json");
    let bindings = write(&fixture.directory, "数据 绑定.json");
    let arguments = [
        "check",
        "recovery",
        "runtime",
        "verify",
        "--source-backend",
        backend.to_str().unwrap(),
        "--source-frontend",
        frontend.to_str().unwrap(),
        "--bindings",
        bindings.to_str().unwrap(),
        "--receipt",
        receipt.to_str().unwrap(),
    ]
    .map(str::to_owned);
    let authority = serde_json::json!({
        "format_version":2, "kind":"restore-runtime-authority", "restore_id":"restore-1",
        "backup_id":"backup-1", "scope_id":"fixture-restore-1", "plan_hash":"a".repeat(64),
        "data_verified_at":"2026-09-06T00:00:00Z", "backup_source_sha":"b".repeat(40),
        "backend_product_sha":"c".repeat(40), "backend_execution_sha":"c".repeat(40),
        "backend_adapter_contract":null, "frontend_sha":"d".repeat(40),
        "api_endpoint":"http://127.0.0.1:18080/readyz",
        "worker_endpoint":"http://127.0.0.1:19091/readyz",
        "frontend_endpoint":"http://127.0.0.1:14174",
    });
    let before = fixture.entries();
    let mut empty = native_command(&arguments, &fixture);
    let empty = empty.stdin(Stdio::null()).output().unwrap();
    assert_eq!(empty.status.code(), Some(1));
    assert!(empty.stdout.is_empty());
    assert!(
        String::from_utf8(empty.stderr)
            .unwrap()
            .contains("verify 必须从 stdin")
    );
    let mut process = native_command(&arguments, &fixture)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    process
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&authority).unwrap())
        .unwrap();
    let invalid = process.wait_with_output().unwrap();
    assert_eq!(invalid.status.code(), Some(1));
    assert!(invalid.stdout.is_empty());
    let stderr = String::from_utf8(invalid.stderr).unwrap();
    assert_eq!(
        stderr.trim_end(),
        "任务失败：恢复运行收据字段缺失或包含未知字段"
    );
    fixture.assert_no_python_hook();
    assert_eq!(fixture.entries(), before);
}

#[test]
fn runtime_start_rejects_invalid_registration_without_python_or_file_changes() {
    let fixture = Fixture::new();
    let backend = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let frontend = fixture.directory.join("前端 来源");
    fs::create_dir(&frontend).unwrap();
    let build = write(&fixture.directory, "后端 构建.json");
    let bindings = write(&fixture.directory, "数据 绑定.json");
    let mut arguments = fixture.runtime_arguments("start");
    arguments.extend([
        "--source-backend".into(),
        backend.to_string_lossy().into_owned(),
        "--source-frontend".into(),
        frontend.to_string_lossy().into_owned(),
        "--build-receipt".into(),
        build.to_string_lossy().into_owned(),
        "--bindings".into(),
        bindings.to_string_lossy().into_owned(),
        "--write".into(),
    ]);

    let before = fixture.entries();
    assert!(fs::read_dir(&frontend).unwrap().next().is_none());
    let result = invoke_with_conflicting_python_environment(&arguments, &fixture);
    assert_native_registration_rejected("start", result);
    fixture.assert_no_python_hook();
    assert_eq!(fixture.entries(), before);
    assert!(fs::read_dir(&frontend).unwrap().next().is_none());
}

#[test]
fn runtime_bind_rejects_invalid_launch_without_python_or_file_changes() {
    let fixture = Fixture::new();
    let backend = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let frontend = fixture.directory.join("前端 来源");
    fs::create_dir(&frontend).unwrap();
    let build = write(&fixture.directory, "后端 构建.json");
    let launch = write(&fixture.directory, "启动 收据.json");
    let bindings = write(&fixture.directory, "数据 绑定.json");
    let output = fixture.directory.join("新 运行收据.json");
    let mut arguments = [
        "check",
        "recovery",
        "runtime",
        "bind",
        "--source-backend",
        backend.to_str().unwrap(),
        "--source-frontend",
        frontend.to_str().unwrap(),
        "--build-receipt",
        build.to_str().unwrap(),
        "--launch-receipt",
        launch.to_str().unwrap(),
        "--bindings",
        bindings.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]
    .map(str::to_owned)
    .to_vec();

    let before = fixture.entries();
    let denied = invoke_with_conflicting_python_environment(&arguments, &fixture);
    assert_eq!(denied.status.code(), Some(2));
    assert!(denied.stdout.is_empty());
    assert!(
        String::from_utf8(denied.stderr)
            .unwrap()
            .contains("--write")
    );

    arguments.push("--write".into());
    let invalid = invoke_with_conflicting_python_environment(&arguments, &fixture);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(invalid.stdout.is_empty());
    assert_eq!(
        String::from_utf8(invalid.stderr).unwrap().trim_end(),
        "任务失败：恢复静态权威路径必须为字符串"
    );
    fixture.assert_no_python_hook();
    assert_eq!(fixture.entries(), before);
    assert!(fs::read_dir(frontend).unwrap().next().is_none());
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
