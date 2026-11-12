//! 夹具控制公开入口的 Rust 参数核验与失败关闭。

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    review: PathBuf,
    environment: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let directory = root.join(".local-tests").join(format!(
            "xtask-fixture-control-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let review = directory.join("review.json");
        let environment = directory.join("bootstrap.json");
        fs::write(&review, b"{}\n").unwrap();
        fs::write(&environment, b"{}\n").unwrap();
        Self {
            directory,
            review,
            environment,
        }
    }

    fn valid(&self) -> Vec<String> {
        vec![
            "check".to_owned(),
            "recovery".to_owned(),
            "fixture".to_owned(),
            "services".to_owned(),
            "status".to_owned(),
            "--review".to_owned(),
            self.review.to_string_lossy().into_owned(),
            "--environment".to_owned(),
            self.environment.to_string_lossy().into_owned(),
        ]
    }

    fn artifact_snapshot(&self) -> Vec<String> {
        vec![
            "check".to_owned(),
            "recovery".to_owned(),
            "fixture".to_owned(),
            "artifact".to_owned(),
            "snapshot".to_owned(),
            "--runtime-dir".to_owned(),
            self.directory.to_string_lossy().into_owned(),
            "--job-id".to_owned(),
            "123".to_owned(),
            "--receipt".to_owned(),
            self.directory
                .join("artifact receipt.json")
                .to_string_lossy()
                .into_owned(),
        ]
    }

    fn retention_inspect(&self) -> Vec<String> {
        vec![
            "check".to_owned(),
            "recovery".to_owned(),
            "fixture".to_owned(),
            "retention".to_owned(),
            "inspect".to_owned(),
            "--runtime-dir".to_owned(),
            self.directory.to_string_lossy().into_owned(),
            "--tenant".to_owned(),
            "tenant-0123abcd".to_owned(),
            "--migration".to_owned(),
            "123".to_owned(),
        ]
    }

    fn dataset_plan(&self) -> Vec<String> {
        vec![
            "check".to_owned(),
            "recovery".to_owned(),
            "fixture".to_owned(),
            "dataset".to_owned(),
            "plan".to_owned(),
            "--environment".to_owned(),
            self.environment.to_string_lossy().into_owned(),
            "--runtime".to_owned(),
            self.directory.to_string_lossy().into_owned(),
            "--work-dir".to_owned(),
            self.directory
                .join("数据 work")
                .to_string_lossy()
                .into_owned(),
            "--output".to_owned(),
            self.directory
                .join("dataset plan.json")
                .to_string_lossy()
                .into_owned(),
            "--side".to_owned(),
            "base".to_owned(),
            "--write".to_owned(),
        ]
    }

    fn dataset_prepare(&self) -> Vec<String> {
        let plan = self.directory.join("dataset plan.json");
        fs::write(&plan, b"{}\n").unwrap();
        vec![
            "check".to_owned(),
            "recovery".to_owned(),
            "fixture".to_owned(),
            "dataset".to_owned(),
            "prepare".to_owned(),
            "--environment".to_owned(),
            self.environment.to_string_lossy().into_owned(),
            "--runtime".to_owned(),
            self.directory.to_string_lossy().into_owned(),
            "--plan".to_owned(),
            plan.to_string_lossy().into_owned(),
            "--side".to_owned(),
            "candidate".to_owned(),
            "--write".to_owned(),
        ]
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn invoke(arguments: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(arguments)
        .env(
            "RYFRAME_REFERENCE_FIXTURE_CONTROL_PROTOCOL",
            "untrusted inherited value",
        )
        .env("PYTHONUTF8", "0")
        .env("PYTHONIOENCODING", "ascii:strict")
        .output()
        .unwrap()
}

#[test]
fn invalid_service_arguments_fail_before_inputs_or_effects() {
    let fixture = Fixture::new();
    let base = fixture.valid();
    let mut cases = Vec::new();
    let mut missing = base.clone();
    missing.truncate(8);
    cases.push(missing);
    let mut duplicate = base.clone();
    duplicate.extend([
        "--review".to_owned(),
        fixture.review.to_string_lossy().into_owned(),
    ]);
    cases.push(duplicate);
    let mut write = base.clone();
    write.push("--write".to_owned());
    cases.push(write);
    let mut outside = base;
    outside[6] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .to_string_lossy()
        .into_owned();
    cases.push(outside);
    for operation in ["rustfs", "redis", "buckets"] {
        let mut initial = fixture.valid();
        initial[4] = operation.to_owned();
        cases.push(initial.clone());
        initial.push("--write".to_owned());
        let mut duplicate = initial.clone();
        duplicate.push("--write".to_owned());
        cases.push(duplicate);
        initial.extend([
            "--owner-binding".to_owned(),
            fixture.review.to_string_lossy().into_owned(),
        ]);
        cases.push(initial);
    }
    for arguments in cases {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(result.stdout.is_empty());
    }
    assert_eq!(fs::read(&fixture.review).unwrap(), b"{}\n");
    assert_eq!(fs::read(&fixture.environment).unwrap(), b"{}\n");
    assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 2);
}

#[test]
fn mutating_services_reject_invalid_environment_without_side_effects() {
    for operation in [
        "rustfs",
        "redis",
        "buckets",
        "close",
        "reconcile",
        "recover",
        "restart",
    ] {
        let fixture = Fixture::new();
        let mut arguments = fixture.valid();
        arguments[4] = operation.to_owned();
        arguments.push("--write".to_owned());
        if matches!(operation, "reconcile" | "recover" | "restart") {
            arguments.extend([
                "--owner-binding".to_owned(),
                fixture.review.to_string_lossy().into_owned(),
            ]);
        }
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(1));
        let stdout = String::from_utf8(result.stdout).unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(stdout.is_empty(), "{stdout}");
        assert!(!stdout.contains("--review"), "{stdout}");
        assert!(
            !stdout.contains(fixture.review.to_str().unwrap()),
            "{stdout}"
        );
        assert!(
            !stdout.contains(fixture.environment.to_str().unwrap()),
            "{stdout}"
        );
        assert!(stderr.contains("任务失败"), "{stderr}");
        assert!(!stderr.contains("protocol_error"), "{stderr}");
        assert_eq!(fs::read(&fixture.review).unwrap(), b"{}\n");
        assert_eq!(fs::read(&fixture.environment).unwrap(), b"{}\n");
        assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 2);
    }
}

#[test]
fn status_uses_native_readonly_validation_and_preserves_original_inputs() {
    let fixture = Fixture::new();
    let original = br#"{"private_marker":"must-not-be-printed"}"#;
    for path in [&fixture.review, &fixture.environment] {
        fs::write(path, original).unwrap();
    }
    let result = invoke(&fixture.valid());
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(!error.trim().is_empty());
    assert!(!error.contains("must-not-be-printed"));
    for path in [&fixture.review, &fixture.environment] {
        assert_eq!(fs::read(path).unwrap(), original);
    }
    assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 2);
}

#[test]
fn source_pair_request_runs_without_python_and_keeps_create_only_output() {
    let fixture = Fixture::new();
    let output = fixture.directory.join("source-pair.json");
    let arguments = vec![
        "check".to_owned(),
        "recovery".to_owned(),
        "fixture".to_owned(),
        "source-pair".to_owned(),
        "--output".to_owned(),
        output.to_string_lossy().into_owned(),
        "--write".to_owned(),
    ];
    let result = invoke(&arguments);
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(!stdout.contains(".py"), "{stdout}");
    assert!(stderr.contains("fixture.json"), "{stderr}");
    assert!(!output.exists());
}

#[test]
fn artifact_snapshot_runs_natively_and_rejects_missing_runtime_before_network() {
    let fixture = Fixture::new();
    let result = invoke(&fixture.artifact_snapshot());
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.contains("runtime.json"), "{stderr}");
    assert!(!stderr.contains(".py"), "{stderr}");
}

#[test]
fn artifact_invalid_arguments_exit_two_before_python_starts() {
    let fixture = Fixture::new();
    let base = fixture.artifact_snapshot();
    let mut cases = Vec::new();
    let mut public_write = base.clone();
    public_write.push("--write".to_owned());
    cases.push(public_write);
    let mut invalid_id = base.clone();
    invalid_id[8] = "0".to_owned();
    cases.push(invalid_id);
    let mut relative_runtime = base.clone();
    relative_runtime[6] = "relative-runtime".to_owned();
    cases.push(relative_runtime);
    let mut duplicate = base;
    duplicate.extend([
        "--receipt".to_owned(),
        fixture.review.to_string_lossy().into_owned(),
    ]);
    cases.push(duplicate);
    cases.push(vec![
        "check".to_owned(),
        "recovery".to_owned(),
        "fixture".to_owned(),
        "artifact".to_owned(),
        "destroy".to_owned(),
        "--help".to_owned(),
    ]);
    for arguments in cases {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("full_stack_artifacts.py"));
    }
}

#[test]
fn artifact_native_process_ignores_legacy_credential_protocol_environment() {
    let fixture = Fixture::new();
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(fixture.artifact_snapshot())
        .env("RYFRAME_REFERENCE_FIXTURE_CONTROL_CONFLICT", "untrusted")
        .env("APP_DATABASE_PASSWORD", "untrusted-secret")
        .env("APP_OBJECT_STORAGE_SECRET_KEY", "untrusted-secret")
        .env("MYSQL_PWD", "untrusted-secret")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("runtime.json"), "{stderr}");
    assert!(!stderr.contains("untrusted-secret"));
}

#[test]
fn retention_native_failure_does_not_forward_arguments_or_credentials() {
    let fixture = Fixture::new();
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(fixture.retention_inspect())
        .env("APP_ENV", "test")
        .env("APP_DATABASE_PASSWORD", "untrusted-secret")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("runtime.json"), "{stderr}");
    assert!(!stderr.contains("untrusted-secret"));
    assert!(!stderr.contains("tools/python"));
}

#[test]
fn retention_invalid_arguments_exit_two_before_python_starts() {
    let fixture = Fixture::new();
    let base = fixture.retention_inspect();
    let mut cases = Vec::new();
    let mut public_write = base.clone();
    public_write.push("--write".to_owned());
    cases.push(public_write);
    let mut invalid_tenant = base.clone();
    invalid_tenant[8] = "tenant-0123ABCD".to_owned();
    cases.push(invalid_tenant);
    let mut invalid_id = base.clone();
    invalid_id[10] = "0".to_owned();
    cases.push(invalid_id);
    let mut duplicate = base;
    duplicate.extend(["--migration".to_owned(), "124".to_owned()]);
    cases.push(duplicate);
    cases.push(vec![
        "check".to_owned(),
        "recovery".to_owned(),
        "fixture".to_owned(),
        "retention".to_owned(),
        "destroy".to_owned(),
        "--help".to_owned(),
    ]);
    for arguments in cases {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(
            !String::from_utf8_lossy(&result.stdout).contains("full_stack_migration_history.py")
        );
    }
}

#[test]
fn native_dataset_invalid_environment_preserves_inputs_and_writes_nothing() {
    for prepare in [false, true] {
        let fixture = Fixture::new();
        let arguments = if prepare {
            fixture.dataset_prepare()
        } else {
            fixture.dataset_plan()
        };
        let before = (
            fs::read(&fixture.environment).unwrap(),
            fs::read(&fixture.review).unwrap(),
            fs::read_dir(&fixture.directory).unwrap().count(),
        );
        let result = invoke(&arguments);
        assert_eq!(
            result.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty());
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(stderr.contains("任务失败"), "{stderr}");
        assert_eq!(
            before,
            (
                fs::read(&fixture.environment).unwrap(),
                fs::read(&fixture.review).unwrap(),
                fs::read_dir(&fixture.directory).unwrap().count(),
            )
        );
    }
}
#[test]
fn dataset_invalid_arguments_exit_two_without_output() {
    let fixture = Fixture::new();
    let base = fixture.dataset_plan();
    let mut cases = Vec::new();
    let mut missing_write = base.clone();
    missing_write.pop();
    cases.push(missing_write);
    let mut seed = base.clone();
    seed[14] = "seed".to_owned();
    cases.push(seed);
    let mut relative_runtime = base.clone();
    relative_runtime[8] = "relative runtime".to_owned();
    cases.push(relative_runtime);
    let mut same_work_and_output = base.clone();
    same_work_and_output[12] = same_work_and_output[10].clone();
    cases.push(same_work_and_output);
    let mut duplicate = base.clone();
    duplicate.extend(["--side".to_owned(), "candidate".to_owned()]);
    cases.push(duplicate);
    let mut public_backend = base;
    public_backend.extend([
        "--backend-dir".to_owned(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    ]);
    cases.push(public_backend);
    for arguments in cases {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(result.stdout.is_empty());
    }
}
