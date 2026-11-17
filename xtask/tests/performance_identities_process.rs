//! 性能身份公开入口拒绝歧义或越界参数；离线计划由原生 Rust 生成。

use serde_json::json;
use sha2::{Digest, Sha256};
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

fn hash(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}

fn private_environment(fixture: &Fixture) -> PathBuf {
    let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let runtime = fixture.directory.join("runtime");
    fs::create_dir(&runtime).unwrap();
    for name in ["runtime.json", "api.json"] {
        fs::write(runtime.join(name), "{}").unwrap();
    }
    let document = fixture.directory.join("environment.md");
    fs::write(&document, "隔离环境").unwrap();
    let connection = |database| json!({"host":"127.0.0.1","port":3306,"username":"fixture","password":"Process-Database-Secret!123","database":database,"tls_mode":"required"});
    let admin = |tenant: &str, index| json!({"tenant_id":tenant,"username":"admin","password":"Process-Admin-Secret!123","client_address":format!("198.18.30.{index}")});
    let value = json!({
        "format_version":1,"plan_id":"process","scope_id":"identity-process","backend_dir":backend,"frontend_dir":backend.parent().unwrap().join("ryframe-vue3"),
        "api_url":"http://127.0.0.1:18999/","frontend_url":"http://127.0.0.1:4199/",
        "environment_document":{"path":document,"sha256":hash("隔离环境")},
        "runtime":{"directory":runtime,"receipt_sha256":hash("{}"),"api_process_sha256":hash("{}")},
        "database":{"mysql_client":backend.join("mysql-fixture"),"server_uuid":"11111111-1111-1111-1111-111111111111","control":connection("identity_control"),"targets":[{"key":"shared","mode":"shared","connection":connection("identity_shared")}]},
        "system_admin":admin("system",1),
        "tenants":(1..=10).map(|index|json!({"slot":format!("tenant-{index:02}"),"tenant_id":format!("fixture-{index}"),"target_key":"shared","admin":admin(&format!("fixture-{index}"),index+1)})).collect::<Vec<_>>(),
        "passwords":{"system":"Process-User-Secret!123","tenant":"Process-User-Secret!123"},
        "quota":{"system_max_users":1000,"tenant_max_users":1000,"import_headroom_per_tenant":100},
        "pacing":{"contract":{"authority_sha256":"a".repeat(64),"request_interval_ms":1000,"operation_intervals_ms":{"post_auth_login":12250},"sample_prepare_wait_ms":60250},"bindings":{"login_budget_state":fixture.directory.join("budget.json")}}
    });
    let path = fixture.directory.join("private.json");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    path
}

#[test]
fn private_plan_and_all_process_outputs_never_persist_or_print_credentials() {
    let fixture = Fixture::new();
    let input = private_environment(&fixture);
    let output = fixture.directory.join("public-plan.json");
    let request = json!({"format_version":1,"operation":"plan","environment":input,"output":output,"write":true});
    let result = private_invoke(&[], &serde_json::to_vec(&request).unwrap());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&output).unwrap();
    let plan: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        plan["configuration"]["sha256"],
        hash(fs::read(&input).unwrap())
    );
    let mut artifacts = vec![bytes, result.stdout, result.stderr];
    for operation in ["apply", "verify"] {
        let payload = json!({"format_version":1,"operation":operation,"plan":output,"state_dir":fixture.directory.join("state"),"write":true});
        let failed = private_invoke(&[], &serde_json::to_vec(&payload).unwrap());
        assert!(!failed.status.success());
        artifacts.extend([failed.stdout, failed.stderr]);
    }
    for secret in [
        "Process-User-Secret!123",
        "Process-Admin-Secret!123",
        "Process-Database-Secret!123",
        "private-secret",
    ] {
        for artifact in &artifacts {
            assert!(!String::from_utf8_lossy(artifact).contains(secret));
        }
    }
    assert!(!fixture.directory.join("state").exists());
}

#[test]
fn native_clone_producer_waits_for_stdin_and_rejects_wrong_grants_before_state_creation() {
    let fixture = Fixture::new();
    let plan = fixture.environment();
    let state = fixture.directory.join("seed-runtime/identities");
    let arguments = vec![
        "__identity-producer".into(),
        "--run-dir".into(),
        fixture.directory.display().to_string(),
        "--attempt".into(),
        "1".into(),
        "--identity-plan".into(),
        plan.display().to_string(),
        "--identity-state".into(),
        state.display().to_string(),
        "--identity-mode".into(),
        "apply".into(),
        "--identity-plan-sha256".into(),
        hash(fs::read(plan).unwrap()),
    ];
    for grant in [
        None,
        Some(
            json!({"operation":"start","run_dir":fixture.directory,"attempt":2,"token":"Process-Reset-Secret!123"}),
        ),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(&arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(child.try_wait().unwrap().is_none());
        assert!(!state.exists());
        if let Some(grant) = grant {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(&grant).unwrap())
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Process-Reset-Secret!123"));
        assert!(!state.exists());
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(child.try_wait().unwrap().is_none());
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!state.exists());
}
