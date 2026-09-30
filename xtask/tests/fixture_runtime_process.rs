//! 夹具运行时公开入口必须在启动 Python 前拒绝歧义，并隐藏路径参数。

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    environment: PathBuf,
    runtime: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let directory = root.join(".local-tests").join(format!(
            "xtask-fixture-runtime-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let environment = directory.join("bootstrap.json");
        fs::write(&environment, b"{}\n").unwrap();
        let runtime = directory.join("runtime-r1");
        fs::create_dir(&runtime).unwrap();
        Self {
            directory,
            environment,
            runtime,
        }
    }

    fn valid(&self) -> Vec<String> {
        vec![
            "check".to_owned(),
            "recovery".to_owned(),
            "fixture".to_owned(),
            "runtime".to_owned(),
            "verify".to_owned(),
            "--environment".to_owned(),
            self.environment.to_string_lossy().into_owned(),
            "--output".to_owned(),
            self.runtime.to_string_lossy().into_owned(),
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
            "RYFRAME_REFERENCE_FIXTURE_RUNTIME_PROTOCOL",
            "untrusted inherited value",
        )
        .env("pythonUtf8", "0")
        .env("pythonIoEncoding", "ascii:strict")
        .output()
        .unwrap()
}

#[test]
fn invalid_public_arguments_exit_two_without_starting_python() {
    let fixture = Fixture::new();
    let base = fixture.valid();
    let mut cases = Vec::new();
    let mut missing = base.clone();
    missing.truncate(7);
    cases.push(missing);
    let mut duplicate = base.clone();
    duplicate.extend([
        "--output".to_owned(),
        fixture.runtime.to_string_lossy().into_owned(),
    ]);
    cases.push(duplicate);
    let mut unknown = base.clone();
    unknown.extend(["--unknown".to_owned(), "value".to_owned()]);
    cases.push(unknown);
    let mut relative = base;
    relative[6] = "bootstrap.json".to_owned();
    cases.push(relative);
    for arguments in cases {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("reference_fixture_runtime.py"));
    }
}

#[test]
fn valid_request_replaces_inherited_protocol_and_keeps_paths_out_of_argv() {
    let fixture = Fixture::new();
    let result = invoke(&fixture.valid());
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stdout.contains("-X utf8 -B tools/python/reference_fixture_runtime.py"),
        "{stdout}"
    );
    assert!(!stdout.contains("--environment"), "{stdout}");
    assert!(
        !stdout.contains(fixture.environment.to_str().unwrap()),
        "{stdout}"
    );
    assert!(
        !stdout.contains(fixture.runtime.to_str().unwrap()),
        "{stdout}"
    );
    assert!(
        stderr.contains("reference_fixture_runtime_failed"),
        "{stderr}"
    );
    assert!(!stderr.contains("runtime_protocol_error"), "{stderr}");
}
