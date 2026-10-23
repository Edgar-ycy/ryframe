//! 性能身份公开入口拒绝歧义或越界参数；离线计划由原生 Rust 生成。

use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let directory = root.join(".local-tests").join(format!(
            "xtask-performance-identities-process-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }

    fn environment(&self) -> PathBuf {
        let value = self.directory.join("environment.json");
        fs::write(&value, b"{}\n").unwrap();
        value
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
            "RYFRAME_PERFORMANCE_IDENTITIES_PROTOCOL",
            "untrusted inherited value",
        )
        .env("RYFRAME_PERFORMANCE_IDENTITIES_EXTRA", "secret-credential")
        .output()
        .unwrap()
}

#[test]
fn invalid_paths_and_missing_write_exit_two_without_starting_node() {
    let fixture = Fixture::new();
    let environment = fixture.environment();
    let output = fixture.directory.join("plan.json");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let base = [
        "data".to_owned(),
        "performance-identities".to_owned(),
        "plan".to_owned(),
        "--environment".to_owned(),
        environment.to_string_lossy().into_owned(),
        "--output".to_owned(),
        output.to_string_lossy().into_owned(),
        "--write".to_owned(),
    ];
    let mut cases = vec![base[..base.len() - 1].to_vec()];
    let mut outside = base.clone();
    outside[6] = root.join("outside.json").to_string_lossy().into_owned();
    cases.push(outside.to_vec());
    let mut relative = base;
    relative[4] = "relative.json".to_owned();
    cases.push(relative.to_vec());
    for arguments in cases {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2), "参数：{arguments:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("参数错误"));
        assert!(!String::from_utf8_lossy(&result.stdout).contains("→ node"));
    }
}

#[test]
fn valid_public_request_uses_stdin_ignores_legacy_environment_and_hides_paths_from_argv() {
    let fixture = Fixture::new();
    let environment = fixture.environment();
    let output = fixture.directory.join("计划 输出.json");
    let arguments = [
        "data".to_owned(),
        "performance-identities".to_owned(),
        "plan".to_owned(),
        "--environment".to_owned(),
        environment.to_string_lossy().into_owned(),
        "--output".to_owned(),
        output.to_string_lossy().into_owned(),
        "--write".to_owned(),
    ];
    let result = invoke(&arguments);
    assert_eq!(result.status.code(), Some(1));
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(!stdout.contains("node"), "{stdout}");
    assert!(!stdout.contains("--environment"), "{stdout}");
    assert!(!stdout.contains(environment.to_str().unwrap()), "{stdout}");
    assert!(stderr.contains("identity_preparation_failed"), "{stderr}");
    assert!(
        !stderr.contains("identity_preparation_protocol_error"),
        "{stderr}"
    );
    assert!(!output.exists());
    assert!(!stdout.contains("secret-credential"));
    assert!(!stderr.contains("secret-credential"));
}

fn private_invoke(arguments: &[&str], input: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__prepare-identities")
        .args(arguments)
        .env("RYFRAME_PERFORMANCE_IDENTITIES_PROTOCOL", "private-secret")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // 超限测试中接收方可以先退出；写端仍应关闭，不能等待下一条协议。
    let _ = child.stdin.take().unwrap().write_all(input);
    child.wait_with_output().unwrap()
}

#[test]
fn private_protocol_rejects_argv_missing_duplicate_and_oversized_input_with_exit_two() {
    for (arguments, input) in [
        (vec!["--help"], b"{}".to_vec()),
        (vec![], vec![]),
        (
            vec![],
            br#"{"format_version":1,"format_\u0076ersion":2}"#.to_vec(),
        ),
        (vec![], vec![b' '; 32 * 1024 + 1]),
        (vec![], vec![0xff]),
        (
            vec![],
            br#"{"format_version":1,"operation":"plan","write":true,"unknown":"private-secret"}"#
                .to_vec(),
        ),
    ] {
        let output = private_invoke(&arguments, &input);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains("identity_preparation_protocol_error"),
            "{error}"
        );
        assert!(!error.contains("private-secret"));
    }
}

#[test]
fn private_plan_uses_native_rust_and_returns_operation_failure_for_invalid_environment() {
    let fixture = Fixture::new();
    let input = fixture.environment();
    let output = fixture.directory.join("身份 输出.json");
    let request = serde_json::json!({"format_version":1,"operation":"plan","environment":input,"output":output,"write":true});
    let result = private_invoke(&[], &serde_json::to_vec(&request).unwrap());
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(error.contains("identity_preparation_failed"));
    assert!(!error.contains("protocol_error"));
    assert!(!output.exists());
}
