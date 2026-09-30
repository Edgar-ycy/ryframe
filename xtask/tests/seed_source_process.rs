//! 正式 seed 来源入口必须在启动 Python 前拒绝歧义或越界参数。

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = backend_root();
        let directory = root.join(".local-tests").join(format!(
            "xtask-seed-source-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
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

fn invoke(arguments: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(arguments)
        .env(
            "RYFRAME_XTASK_RECOVERY_SEED_SOURCE",
            "untrusted inherited seed value",
        )
        .env(
            "RYFRAME_XTASK_RECOVERY_FRESH_TARGET",
            "untrusted inherited fresh value",
        )
        .output()
        .unwrap()
}

fn status_arguments(run_dir: &std::path::Path) -> Vec<String> {
    vec![
        "check".to_owned(),
        "recovery".to_owned(),
        "clone".to_owned(),
        "seed-runtime".to_owned(),
        "--run-dir".to_owned(),
        run_dir.to_string_lossy().into_owned(),
        "--operation".to_owned(),
        "source-generation-status".to_owned(),
    ]
}

#[test]
fn malformed_or_unsafe_requests_exit_two_before_starting_python() {
    let fixture = Fixture::new();
    let valid = status_arguments(&fixture.directory);
    let mut outside = valid.clone();
    outside[5] = backend_root()
        .join("scripts")
        .to_string_lossy()
        .into_owned();
    let mut relative = valid.clone();
    relative[5] = ".local-tests/relative".to_owned();
    let mut duplicate = valid.clone();
    duplicate.extend(["--operation".to_owned(), "source-export".to_owned()]);
    let mut write = valid;
    write.push("--write".to_owned());
    for arguments in [outside, relative, duplicate, write] {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("→ python"));
    }
}

#[test]
fn valid_request_replaces_inherited_protocol_and_hides_paths_from_argv() {
    let fixture = Fixture::new();
    let arguments = status_arguments(&fixture.directory);
    let result = invoke(&arguments);
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stdout.contains("tools/python/devex_clone.py"), "{stdout}");
    assert!(!stdout.contains("--run-dir"), "{stdout}");
    assert!(
        !stdout.contains(fixture.directory.to_str().unwrap()),
        "{stdout}"
    );
    assert!(stderr.contains("任务失败"), "{stderr}");
    assert!(!stderr.contains("私有协议无效"), "{stderr}");
}
