//! 复制入口必须在启动 Python 前完成严格解析，并且不通过 argv 暴露业务参数。

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = backend_root().join(".local-tests").join(format!(
            "xtask-clone-process-{}-{}-中文路径",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.directory
                .starts_with(backend_root().join(".local-tests"))
        );
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn backend_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn invoke(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(arguments)
        .env("RYFRAME_XTASK_RECOVERY_CLONE", "untrusted clone value")
        .env("RYFRAME_XTASK_RECOVERY_SEED_SOURCE", "untrusted seed value")
        .env(
            "RYFRAME_XTASK_RECOVERY_FRESH_TARGET",
            "untrusted fresh value",
        )
        .output()
        .unwrap()
}

fn status_arguments(run_dir: &Path) -> Vec<String> {
    vec![
        "check".to_owned(),
        "recovery".to_owned(),
        "clone".to_owned(),
        "status".to_owned(),
        "--run-dir".to_owned(),
        run_dir.to_string_lossy().into_owned(),
    ]
}

#[test]
fn malformed_requests_exit_two_before_starting_python() {
    let fixture = Fixture::new();
    let valid = status_arguments(&fixture.directory);
    let mut outside = valid.clone();
    outside[5] = backend_root()
        .join("scripts")
        .to_string_lossy()
        .into_owned();
    let mut duplicate = valid.clone();
    duplicate.extend([
        "--run-dir".to_owned(),
        fixture.directory.to_string_lossy().into_owned(),
    ]);
    let mut write = valid.clone();
    write.push("--write".to_owned());
    let mut relative = valid.clone();
    relative[5] = ".local-tests/relative".to_owned();
    let mut unknown = valid.clone();
    unknown.push("--unknown".to_owned());
    let missing = status_arguments(&fixture.directory.join("missing"));
    for arguments in [outside, duplicate, write, relative, unknown, missing] {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("→ python"));
    }
}

#[test]
fn valid_request_replaces_inherited_protocol_and_hides_parameters_from_argv() {
    let fixture = Fixture::new();
    let arguments = status_arguments(&fixture.directory);
    let result = invoke(&arguments);
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stdout.contains("scripts/devex_clone.py"), "{stdout}");
    assert!(!stdout.contains("--run-dir"), "{stdout}");
    assert!(
        !stdout.contains(fixture.directory.to_str().unwrap()),
        "{stdout}"
    );
    assert!(stderr.contains("任务失败"), "{stderr}");
    assert!(!stderr.contains("私有协议无效"), "{stderr}");
}
