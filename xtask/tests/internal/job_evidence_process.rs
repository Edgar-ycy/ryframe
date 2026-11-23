use std::{
    ffi::OsString,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    devex::job_evidence::mysql::{MysqlSession, invocation::Environment},
    mysql_verification_client::{binary_name, compile_client},
    recovery::clone::native::resources::common,
};

struct Fixture {
    directory: Option<tempfile::TempDir>,
    executable: PathBuf,
}

impl Fixture {
    fn compile() -> Self {
        let root = crate::workspace::root_dir().join(".local-tests/job-evidence-process");
        fs::create_dir_all(&root).unwrap();
        let directory = tempfile::tempdir_in(root).unwrap();
        let executable = directory.path().join(binary_name("mysql-fixture"));
        compile_client(&executable);
        Self {
            directory: Some(directory),
            executable,
        }
    }

    fn binding(&self, mode: &str) -> Value {
        let path = self
            .directory
            .as_ref()
            .unwrap()
            .path()
            .join(binary_name(mode));
        fs::copy(&self.executable, &path).unwrap();
        json!({"control": {"host": "127.0.0.1", "port": 3306, "database": "fixture_control",
            "username": "fixture", "password": "fixture-only", "tls_mode": "required"},
            "server_uuid": "12345678-1234-1234-1234-123456789abc", "mysql_client": path})
    }

    fn open(&self, mode: &str) -> MysqlSession {
        let binding = self.binding(mode);
        let mut environment = Environment::new();
        for name in ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                environment.insert(OsString::from(name), value);
            }
        }
        for (key, value) in [
            ("FIXTURE_PASSWORD", "wrong-ambient-secret"),
            ("MYSQL_PWD", "wrong-ambient-secret"),
            ("mysql_pwd", "another-ambient-secret"),
            ("GITHUB_ACTIONS", "true"),
            ("APP_SCOPE_ID", "wrong-scope"),
            ("RYFRAME_CI_MYSQL_CONTAINER_ID", "not-a-container"),
        ] {
            environment.insert(key.into(), value.into());
        }
        MysqlSession::open_with_environment(&binding, &environment).expect("应打开受限原生夹具")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(directory) = self.directory.take()
        {
            eprintln!(
                "原生 MySQL 会话测试现场保留于 {}",
                directory.keep().display()
            );
        }
    }
}

fn identity(session: &mut MysqlSession) -> Value {
    let rows = session
        .execute("SELECT fixture_identity", Duration::from_secs(5))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["password"], true);
    assert_eq!(rows[0]["private_removed"], true);
    let path = PathBuf::from(rows[0]["options"].as_str().unwrap());
    assert!(path.is_file(), "进程存活时保留凭据文件");
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "[client]\npassword=\"fixture-only\"\n"
    );
    let pid = u32::try_from(rows[0]["pid"].as_u64().unwrap()).unwrap();
    common::process_identity(pid)
        .unwrap()
        .expect("夹具进程应存活")
}

fn assert_stopped(identity: &Value) {
    let pid = u32::try_from(identity["pid"].as_u64().unwrap()).unwrap();
    assert_ne!(
        common::process_identity(pid).unwrap().as_ref(),
        Some(identity),
        "原客户端内核代次必须已经停止"
    );
}

fn assert_options_removed(fixture: &Fixture, mode: &str) {
    let marker = fixture
        .directory
        .as_ref()
        .unwrap()
        .path()
        .join(binary_name(mode))
        .with_extension("options");
    let path = PathBuf::from(fs::read_to_string(marker).unwrap());
    assert!(!path.exists(), "关闭后删除凭据文件");
    assert!(!path.parent().unwrap().exists(), "关闭后删除独占目录");
}

fn healthy_sessions(fixture: &Fixture) {
    for mode in ["mysql-normal", "mysql-warning"] {
        let mut session = fixture.open(mode);
        let first = identity(&mut session);
        let second = identity(&mut session);
        assert_eq!(first, second, "连续查询必须使用同一真实进程与连接");
        let rows = session
            .execute("SELECT fixture_stderr", Duration::from_secs(5))
            .unwrap();
        assert_eq!(rows[0]["pid"], first["pid"]);
        session
            .close()
            .expect("警告与被完整排空的stderr不应使正常关闭失败");
        let rollback = fixture
            .directory
            .as_ref()
            .unwrap()
            .path()
            .join(binary_name(mode))
            .with_extension("rollback");
        assert_eq!(
            fs::read(rollback).unwrap(),
            b"observed",
            "健康关闭应先回滚只读事务"
        );
        assert_stopped(&first);
        assert_eq!(
            fs::read(
                fixture
                    .directory
                    .as_ref()
                    .unwrap()
                    .path()
                    .join(binary_name(mode))
                    .with_extension("option-at-exit")
            )
            .unwrap(),
            b"observed"
        );
        assert_options_removed(fixture, mode);
        session.close().expect("关闭后重复关闭应幂等");
    }
}

fn failed_queries(fixture: &Fixture) {
    for (query, expected) in [
        ("SELECT fixture_utf8", Some("mysql-output-utf8")),
        ("SELECT fixture_limit", Some("mysql-output-limit")),
        ("SELECT fixture_duplicate", Some("mysql-output-json")),
        ("SELECT fixture_password_error", Some("mysql-output-json")),
        ("SELECT fixture_exit", None),
        ("SELECT fixture_timeout", Some("mysql-timeout")),
    ] {
        let mut session = fixture.open("mysql-failure");
        let before = identity(&mut session);
        let error = session
            .execute(query, Duration::from_millis(200))
            .unwrap_err();
        if let Some(expected) = expected {
            assert_eq!(error, expected);
        } else {
            assert!(matches!(
                error,
                "mysql-process-exit" | "mysql-eof" | "mysql-reader-stopped"
            ));
        }
        assert_eq!(
            session.execute("SELECT fixture_identity", Duration::from_secs(1)),
            Err("mysql-closed")
        );
        assert_stopped(&before);
        assert_options_removed(fixture, "mysql-failure");
        drop(session);
    }
}

fn blocked_pipe_and_close_timeout(fixture: &Fixture) {
    let mut session = fixture.open("mysql-block");
    let started = Instant::now();
    let sql = format!("SELECT {}", "x".repeat(1024 * 1024));
    assert_eq!(
        session.execute(&sql, Duration::from_millis(200)),
        Err("mysql-timeout")
    );
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "stdin阻塞不得阻止超时回收"
    );
    drop(session);
    assert_options_removed(fixture, "mysql-block");
    let mut session = fixture.open("mysql-close-hang");
    let before = identity(&mut session);
    let started = Instant::now();
    assert_eq!(session.close(), Err("mysql-close-timeout"));
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "关闭超时后必须回收完整客户端"
    );
    assert_stopped(&before);
    assert_options_removed(fixture, "mysql-close-hang");
}

fn cleanup_failure_closes_process_and_refuses_success(fixture: &Fixture) {
    let mut session = fixture.open("mysql-cleanup-failure");
    let before = identity(&mut session);
    let marker = fixture
        .directory
        .as_ref()
        .unwrap()
        .path()
        .join(binary_name("mysql-cleanup-failure"))
        .with_extension("options");
    let path = PathBuf::from(fs::read_to_string(marker).unwrap());
    let foreign = path.parent().unwrap().join("unexpected");
    fs::write(&foreign, b"unrelated").unwrap();
    assert_eq!(session.close(), Err("mysql-option-cleanup"));
    assert_stopped(&before);
    assert!(!path.exists());
    assert!(foreign.is_file());
    fs::remove_file(foreign).unwrap();
    assert_eq!(session.close(), Err("mysql-closed"));
    assert_options_removed(fixture, "mysql-cleanup-failure");
}

#[test]
fn native_mysql_session_preserves_connection_and_recovers_all_pipe_lifecycles() {
    let fixture = Fixture::compile();
    healthy_sessions(&fixture);
    failed_queries(&fixture);
    blocked_pipe_and_close_timeout(&fixture);
    cleanup_failure_closes_process_and_refuses_success(&fixture);
    actual_private_entrypoints(&fixture);
    let mut session = fixture.open("mysql-drop");
    let before = identity(&mut session);
    drop(session);
    assert_stopped(&before);
    assert_options_removed(&fixture, "mysql-drop");
    assert!(fixture.executable.is_file());
}

fn invoke(operation: &str, input: &[u8]) -> std::process::Output {
    crate::process::command_output_with_input_timeout(
        &crate::workspace::root_dir(),
        env!("CARGO_BIN_EXE_xtask"),
        &["__job-evidence", operation],
        &[
            ("FIXTURE_PASSWORD", "wrong-ambient-secret"),
            ("MYSQL_PWD", "wrong-ambient-secret"),
            ("GITHUB_ACTIONS", "true"),
        ],
        Some(input),
        Duration::from_secs(15),
    )
    .expect("原生入口应在完整进程树约束下退出")
}

fn actual_private_entrypoints(fixture: &Fixture) {
    let binding = fixture.binding("mysql-native-warning");
    let scope = "devex-test-scope";
    let selection = json!({"kind":"export", "id":"101", "tenant_id":"tenant-a", "job_type":"system.export.execute"});
    let request = json!({"binding":binding, "scope_id":scope, "selection":selection});
    let output = invoke("collect", &serde_json::to_vec(&request).unwrap());
    assert!(
        output.status.success(),
        "原生采集入口失败：{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    let proof: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(proof["format_version"], 1);
    assert_eq!(proof["valid"], true);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-only"));
    assert_options_removed(fixture, "mysql-native-warning");
    assert_eq!(proof["scope_id"], scope);
    assert_eq!(proof["business_id"], "101");
    assert_eq!(proof["queue_ms"].as_f64(), Some(1000.0));
    assert_eq!(proof["execution_ms"].as_f64(), Some(2000.0));
    let mut header = serde_json::to_vec(&json!({"binding":binding, "scope_id":scope})).unwrap();
    header.push(b'\n');
    let output = invoke("wait", &header);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"format_version":1,"ready":true,"scope_id":scope})
    );
    for input in [b"not-json".as_slice(), br#"{"binding":{},"binding":{}}"#] {
        let output = invoke("collect", input);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"format_version":1,"valid":false,"reason":"invalid_collector_request", "hint":"核对同一版本控制器产生的请求，检查输入是否完整以及大小限制。"})
        );
    }
    let mut request = request;
    request["binding"] = fixture.binding("mysql-native-query-error");
    let output = invoke("collect", &serde_json::to_vec(&request).unwrap());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    assert_options_removed(fixture, "mysql-native-query-error");
    let failure: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(failure["valid"], false);
    assert!(failure["reason"].as_str().unwrap().starts_with("mysql-"));
    assert!(!failure["hint"].as_str().unwrap().is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-private-credential"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-only"));
}
