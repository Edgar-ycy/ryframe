//! 夹具运行时公开入口拒绝歧义，冻结输入失败不启动进程或泄漏私有正文。

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
        .output()
        .unwrap()
}

#[test]
fn invalid_public_arguments_exit_two_without_launching_runtime() {
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
    }
}

#[test]
fn invalid_frozen_environment_is_rejected_without_launch_or_private_input_disclosure() {
    let fixture = Fixture::new();
    let input = b"{\"private\":\"SyntheticCredentialForFailure\"}\n";
    fs::write(&fixture.environment, input).unwrap();
    let result = invoke(&fixture.valid());
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.contains("已准备私有环境"), "{stderr}");
    for value in [&stdout, &stderr] {
        assert!(!value.contains("SyntheticCredentialForFailure"));
        assert!(!value.contains(fixture.environment.to_str().unwrap()));
        assert!(!value.contains(fixture.runtime.to_str().unwrap()));
    }
    assert_eq!(fs::read(&fixture.environment).unwrap(), input);
    assert!(fs::read_dir(&fixture.runtime).unwrap().next().is_none());
}
