use super::super::backup::Exporter;
use super::*;
use serde_json::json;
use std::{sync::OnceLock, thread, time::Instant};

fn fixture() -> &'static Path {
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE
        .get_or_init(|| {
            let root = crate::workspace::root_dir().join(".local-tests");
            fs::create_dir_all(&root).unwrap();
            let directory = tempfile::tempdir_in(root).unwrap();
            let executable = directory
                .path()
                .join(format!("mysql{}", std::env::consts::EXE_SUFFIX));
            let mut command = Command::new("rustc");
            command
                .args([
                    "--edition",
                    "2024",
                    "-C",
                    "debuginfo=0",
                    "--crate-name",
                    "retention_fixture",
                ])
                .arg(crate::workspace::root_dir().join("xtask/tests/fixtures/retention_client.rs"))
                .arg("-o")
                .arg(&executable);
            if cfg!(windows) {
                command.args(["-C", "linker=rust-lld.exe"]);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            fs::copy(
                &executable,
                directory
                    .path()
                    .join(format!("mysqldump{}", std::env::consts::EXE_SUFFIX)),
            )
            .unwrap();
            (directory, executable)
        })
        .1
}
fn connection(directory: &Path, username: &str) -> Connection {
    Connection {
        value: json!({"host":"127.0.0.1","port":3306,"database":"fixture_target","username":username,"password":"fixture-only","tls_mode":"required"}),
        schema: "fixture_target".into(),
        client: Client::Local(fixture().into()),
        options_root: directory.join("mysql-options"),
    }
}
fn temporary() -> tempfile::TempDir {
    let root = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
fn gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if crate::recovery::clone::native::resources::common::process_identity(pid)
            .unwrap()
            .is_none()
        {
            return;
        }
        assert!(Instant::now() < deadline, "进程 {pid} 未回收");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn native_session_reads_file_credentials_and_reclaims_orphan_tree() {
    let directory = temporary();
    let binding = connection(directory.path(), "fixture");
    let invocation = binding.invocation(false).unwrap();
    let mut session = Mysql::open_command(
        invocation.command,
        invocation.options,
        b"[client]\npassword=\"fixture-only\"\n",
        Cancellation::default(),
        Duration::from_secs(5),
    )
    .unwrap();
    let identity = query(&mut session, "SELECT fixture_identity").unwrap();
    assert_eq!(identity[0]["credential"], true);
    assert_eq!(identity[0]["environment"], true);
    let parent = identity[0]["pid"].as_u64().unwrap() as u32;
    let child = query(&mut session, "SELECT fixture_orphan").unwrap()[0]["pid"]
        .as_u64()
        .unwrap() as u32;
    drop(session);
    gone(parent);
    gone(child);
    assert_eq!(
        fs::read_dir(directory.path().join("mysql-options"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn native_session_crash_timeout_and_cancellation_are_redacted_and_closed() {
    for operation in ["SELECT fixture_crash", "SELECT fixture_wait", "cancel"] {
        let directory = temporary();
        let binding = connection(directory.path(), "fixture");
        let invocation = binding.invocation(false).unwrap();
        let options = invocation.options.path().to_path_buf();
        let cancel = Cancellation::default();
        let mut session = Mysql::open_command(
            invocation.command,
            invocation.options,
            &[],
            cancel.clone(),
            Duration::from_millis(100),
        )
        .unwrap();
        let pid = query(&mut session, "SELECT fixture_identity").unwrap()[0]["pid"]
            .as_u64()
            .unwrap() as u32;
        if operation == "cancel" {
            cancel.cancel();
        }
        let error = query(&mut session, operation).unwrap_err().to_string();
        assert!(!error.contains("fixture-only"));
        assert!(error.contains("禁止自动重放"));
        assert!(query(&mut session, "SELECT fixture_identity").is_err());
        drop(session);
        gone(pid);
        assert!(!options.exists());
    }
}

#[test]
fn actual_dump_requires_exit_success_and_keeps_failed_or_incomplete_files() {
    for mode in ["fixture", "failed", "incomplete"] {
        let directory = temporary();
        let mut binding = connection(directory.path(), mode);
        let result = binding.export(directory.path(), 123);
        let path = directory.path().join("business-retention-123.sql");
        assert!(path.is_file());
        if mode == "fixture" {
            let receipt = result.unwrap();
            assert_eq!(receipt["restored"], false);
            super::super::backup::verify(&receipt, directory.path(), 123).unwrap();
        } else {
            assert!(!result.unwrap_err().to_string().contains("fixture-only"));
        }
        assert!(binding.export(directory.path(), 123).is_err());
    }
}

#[test]
fn transport_command_never_inherits_credential_values_and_container_uses_stdin() {
    let directory = temporary();
    let mut binding = connection(directory.path(), "fixture");
    let invocation = binding.invocation(false).unwrap();
    let command = format!("{:?}", invocation.command);
    assert!(!command.contains("fixture-only"));
    assert!(invocation.prefix.is_empty());
    binding.client = Client::Container("a".repeat(12));
    let invocation = binding.invocation(false).unwrap();
    assert_eq!(invocation.prefix, b"[client]\npassword=\"fixture-only\"\n");
    assert!(!format!("{:?}", invocation.command).contains("fixture-only"));
    for (key, value) in [
        ("host", json!("example.com")),
        ("tls_mode", json!("verify_identity")),
        ("port", json!(0)),
        ("password_env", json!("OLD_PASSWORD")),
    ] {
        let mut invalid = binding.value.clone();
        invalid[key] = value;
        assert!(validate(&invalid).is_err(), "{key}");
    }
}
