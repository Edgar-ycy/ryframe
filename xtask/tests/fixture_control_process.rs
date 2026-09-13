//! 夹具控制公开入口必须先完成 Rust 参数核验，并只传递单个私有协议。

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
fn invalid_arguments_exit_two_before_python_starts() {
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
    for arguments in cases {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("reference_fixture_services.py"));
    }
}

#[test]
fn valid_request_replaces_inherited_protocol_and_hides_paths_from_argv() {
    let fixture = Fixture::new();
    let result = invoke(&fixture.valid());
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("scripts/reference_fixture_services.py"),
        "{stdout}"
    );
    assert!(!stdout.contains("--review"), "{stdout}");
    assert!(
        !stdout.contains(fixture.review.to_str().unwrap()),
        "{stdout}"
    );
    assert!(
        !stdout.contains(fixture.environment.to_str().unwrap()),
        "{stdout}"
    );
    assert!(
        stderr.contains("reference_fixture_services_failed"),
        "{stderr}"
    );
    assert!(!stderr.contains("protocol_error"), "{stderr}");
}

#[test]
fn source_pair_request_uses_the_same_private_protocol_without_forwarded_paths() {
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
    assert!(
        stdout.contains("scripts/reference_fixture_source_pair.py"),
        "{stdout}"
    );
    assert!(!stdout.contains("--output"), "{stdout}");
    assert!(!stdout.contains(output.to_str().unwrap()), "{stdout}");
    assert!(
        stderr.contains("reference_fixture_source-pair_failed"),
        "{stderr}"
    );
    assert!(!output.exists());
}

#[test]
fn artifact_snapshot_uses_private_protocol_without_public_write_or_forwarded_values() {
    let fixture = Fixture::new();
    let arguments = fixture.artifact_snapshot();
    let result = invoke(&arguments);
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("scripts/full_stack_artifacts.py"),
        "{stdout}"
    );
    for private in ["--runtime-dir", "--job-id", "--receipt", "123"] {
        assert!(!stdout.contains(private), "{stdout}");
    }
    assert!(
        stderr.contains("reference_fixture_artifact_failed"),
        "{stderr}"
    );
    assert!(!stderr.contains("protocol_error"), "{stderr}");
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
fn artifact_private_process_rejects_conflicting_fixture_protocol_environment() {
    let fixture = Fixture::new();
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(fixture.artifact_snapshot())
        .env("RYFRAME_REFERENCE_FIXTURE_CONTROL_CONFLICT", "untrusted")
        .env("PYTHONUTF8", "1")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("protocol_error"));
}

#[test]
fn retention_request_uses_private_protocol_without_forwarded_values() {
    let fixture = Fixture::new();
    let result = invoke(&fixture.retention_inspect());
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("scripts/full_stack_migration_history.py"),
        "{stdout}"
    );
    for private in [
        "--runtime-dir",
        "--tenant",
        "--migration",
        "tenant-0123abcd",
        "123",
    ] {
        assert!(!stdout.contains(private), "{stdout}");
    }
    assert!(
        stderr.contains("reference_fixture_retention_failed"),
        "{stderr}"
    );
    assert!(!stderr.contains("protocol_error"), "{stderr}");
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
fn dataset_plan_uses_private_protocol_without_forwarded_values() {
    let fixture = Fixture::new();
    let result = invoke(&fixture.dataset_plan());
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("scripts/reference_fixture_dataset.py"),
        "{stdout}"
    );
    assert!(
        stdout.contains("-X utf8 -B scripts/reference_fixture_dataset.py"),
        "{stdout}"
    );
    for private in [
        "--environment",
        "--runtime",
        "--work-dir",
        "--output",
        "--side",
        "数据 work",
        "base",
    ] {
        assert!(!stdout.contains(private), "{stdout}");
    }
    assert!(
        stderr.contains("reference_fixture_dataset_failed"),
        "{stderr}"
    );
    assert!(!stderr.contains("protocol_error"), "{stderr}");
}

#[test]
fn dataset_prepare_uses_private_protocol_without_forwarded_values() {
    let fixture = Fixture::new();
    let result = invoke(&fixture.dataset_prepare());
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("scripts/reference_fixture_dataset.py"),
        "{stdout}"
    );
    for private in [
        "--environment",
        "--runtime",
        "--plan",
        "--side",
        "dataset plan.json",
        "candidate",
    ] {
        assert!(!stdout.contains(private), "{stdout}");
    }
    assert!(
        stderr.contains("reference_fixture_dataset_failed"),
        "{stderr}"
    );
    assert!(!stderr.contains("protocol_error"), "{stderr}");
}

#[test]
fn dataset_invalid_arguments_exit_two_before_python_starts() {
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
        assert!(!String::from_utf8_lossy(&result.stdout).contains("reference_fixture_dataset.py"));
    }
}
