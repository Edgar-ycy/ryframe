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
    let mut unknown = valid;
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
fn post_copy_verify_rejects_substituted_inputs_before_execution() {
    let fixture = Fixture::new();
    let input = fixture.directory.join("input.json");
    fs::write(&input, b"{}").unwrap();
    let run_dir = fixture.directory.to_str().unwrap();
    let input_path = input.to_str().unwrap();

    for flag in ["--request", "--producer-binding"] {
        let arguments = [
            "check",
            "recovery",
            "clone",
            "post-copy",
            "--run-dir",
            run_dir,
            "--operation",
            "verify",
            flag,
            input_path,
            "--write",
        ]
        .map(str::to_owned);
        let result = invoke(&arguments);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.code(), Some(2), "{flag}: {stderr}");
        assert!(
            stderr.contains("post-copy 的 request 或 producer binding 与操作不匹配"),
            "{flag}: {stderr}"
        );
        assert!(result.stdout.is_empty(), "{flag}");
    }
    let files = fs::read_dir(&fixture.directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(files, [input.clone()]);
    assert_eq!(fs::read(input).unwrap(), b"{}");
}

#[test]
fn valid_status_uses_native_path_and_ignores_inherited_protocol() {
    let fixture = Fixture::new();
    let arguments = status_arguments(&fixture.directory);
    let result = invoke(&arguments);
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!stdout.contains("tools/python/devex_clone.py"), "{stdout}");
    assert!(!stdout.contains("--run-dir"), "{stdout}");
    assert!(
        !stdout.contains(fixture.directory.to_str().unwrap()),
        "{stdout}"
    );
    assert!(stderr.contains("任务失败"), "{stderr}");
    assert!(!stderr.contains("私有协议无效"), "{stderr}");
    assert!(!stderr.contains("untrusted clone value"), "{stderr}");
    assert!(!stderr.contains("untrusted seed value"), "{stderr}");
    assert!(!stderr.contains("untrusted fresh value"), "{stderr}");
}

#[test]
fn offline_plan_and_verify_fail_without_starting_python() {
    let fixture = Fixture::new();
    let input = fixture.directory.join("input.json");
    fs::write(&input, b"{}").unwrap();
    let output = fixture.directory.join("plan.json");
    for arguments in [
        vec![
            "check".to_owned(),
            "recovery".to_owned(),
            "clone".to_owned(),
            "plan".to_owned(),
            "--input".to_owned(),
            input.to_string_lossy().into_owned(),
            "--output".to_owned(),
            output.to_string_lossy().into_owned(),
            "--write".to_owned(),
        ],
        vec![
            "check".to_owned(),
            "recovery".to_owned(),
            "clone".to_owned(),
            "verify".to_owned(),
            "--plan".to_owned(),
            input.to_string_lossy().into_owned(),
        ],
    ] {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(1));
        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!stdout.contains("tools/python/devex_clone.py"), "{stdout}");
        assert!(!stdout.contains("→ python"), "{stdout}");
        assert!(!stderr.contains("tools/python/devex_clone.py"), "{stderr}");
    }
    assert!(!output.exists());
}
