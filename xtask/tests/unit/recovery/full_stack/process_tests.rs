use super::*;
use std::{path::PathBuf, sync::OnceLock};

fn directory() -> tempfile::TempDir {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    fs::create_dir(directory.path().join(".local-tests")).unwrap();
    directory
}
fn fixture() -> &'static Path {
    static VALUE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &VALUE
        .get_or_init(|| {
            let directory = directory();
            let executable = directory
                .path()
                .join(format!("协调器 替身{}", std::env::consts::EXE_SUFFIX));
            let mut command = Command::new("rustc");
            command
                .args([
                    "--edition",
                    "2024",
                    "-C",
                    "debuginfo=0",
                    "--crate-name",
                    "full_stack_ci_process",
                ])
                .arg(
                    crate::workspace::root_dir()
                        .join("xtask/tests/fixtures/full_stack_ci_process.rs"),
                )
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
            (directory, executable)
        })
        .1
}
fn command(mode: &str, directory: &Path) -> Command {
    let mut command = Command::new(fixture());
    command.arg(mode).arg(directory);
    command
}
fn gone(directory: &Path) {
    for name in ["parent.pid", "child.pid"] {
        let path = directory.join(name);
        if !path.exists() {
            continue;
        }
        let pid = fs::read_to_string(path).unwrap().parse().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while crate::recovery::clone::native::resources::common::process_identity(pid)
            .unwrap()
            .is_some()
        {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn captures_private_stdin_without_putting_values_in_arguments_or_errors() {
    let root = directory();
    let output = capture(
        command("echo", root.path()),
        root.path(),
        b"private-password",
        Duration::from_secs(5),
        None,
        false,
        false,
    )
    .unwrap();
    assert_eq!(output, "private-password");
    for (mode, compiler_failure) in [("fail", false), ("compiler-fail", true)] {
        let error = capture(
            command(mode, root.path()),
            root.path(),
            b"private-password",
            Duration::from_secs(5),
            None,
            false,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("17"));
        assert!(!error.contains("private-password"));
        assert_eq!(error.contains("generator.target_dir"), compiler_failure);
    }
    gone(root.path());
}
#[test]
fn cancellation_timeout_and_successful_orphan_exit_reclaim_complete_managed_tree() {
    for mode in ["cancel", "timeout", "orphan"] {
        let root = directory();
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        let path = root.path().to_path_buf();
        let monitor = thread::spawn(move || {
            if mode == "cancel" {
                let until = Instant::now() + Duration::from_secs(10);
                while !path.join("child.pid").is_file() {
                    assert!(Instant::now() < until);
                    thread::sleep(Duration::from_millis(10));
                }
                trigger.store(true, Ordering::Release);
            }
        });
        let result = capture(
            command(
                if mode == "orphan" { "orphan" } else { "hang" },
                root.path(),
            ),
            root.path(),
            b"",
            Duration::from_secs(1),
            Some(&cancel),
            false,
            false,
        );
        monitor.join().unwrap();
        assert_eq!(result.is_ok(), mode == "orphan");
        gone(root.path());
    }
}
#[test]
fn output_limit_fails_closed_and_never_leaks_processes() {
    let root = directory();
    assert!(
        capture(
            command("flood", root.path()),
            root.path(),
            b"",
            Duration::from_secs(5),
            None,
            false,
            false,
        )
        .is_err()
    );
    gone(root.path());
}
#[test]
fn logging_redacts_nested_plaintext_credentials_and_preserves_other_evidence() {
    let value = json!({"database":{"password":"private-password"},"tenant_data":{"targets":[{"password":"tenant-secret"}]},"auth":{"jwt_secret":"jwt-key"}});
    let text = redact(
        "phase reset private-password tenant-secret jwt-key".into(),
        &value,
    );
    assert_eq!(text, "phase reset [已脱敏] [已脱敏] [已脱敏]");
}

#[test]
fn private_cargo_json_never_merges_compiler_diagnostics_into_artifact_events() {
    let root = directory();
    let output = capture(
        command("json", root.path()),
        root.path(),
        b"",
        Duration::from_secs(5),
        None,
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap(),
        json!({"reason":"compiler-artifact"})
    );
    assert!(!output.contains("private-password"));
    gone(root.path());
}

#[test]
fn bound_local_storage_configuration_creates_only_business_buckets_and_preserves_files() {
    let root = directory();
    let context = Context {
        root: root.path().into(),
        output: root.path().into(),
        control: json!({}),
        run: String::new(),
        attempt: String::new(),
    };
    let mut native = Native {
        context: &context,
        cancel: Arc::new(AtomicBool::new(false)),
        api: None,
    };
    let storage = root.path().join("绑定配置 本地对象");
    let table = json!({"object_storage":{"backend":"local","local_base_dir":storage,
        "endpoint":"invalid-unused-endpoint","access_key":"unused","secret_key":"unused"}});
    native.storage(&table).unwrap();
    let mut actual = fs::read_dir(&storage)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    actual.sort();
    assert_eq!(
        actual,
        ["avatar", "config-packages", "exports", "imports", "uploads"]
    );
    let existing = storage.join("uploads/existing.txt");
    fs::write(&existing, b"keep").unwrap();
    native.storage(&table).unwrap();
    assert_eq!(fs::read(existing).unwrap(), b"keep");
    assert!(!root.path().join("ci-storage.json").exists());
}

#[test]
fn bound_s3_credentials_are_literal_and_bucket_failures_never_retry_or_expose_secrets() {
    use crate::recovery::clone::native::s3;
    use std::net::TcpListener;
    for (code, expected, unknown) in [
        (403, "denied", false),
        (409, "already_exists", false),
        (503, "needs_reconciliation", true),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(10);
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(value) => break value,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < until =>
                    {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("隔离对象协议替身未收到请求：{error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).unwrap();
                assert!(count > 0 && request.len() < 32768);
                request.extend_from_slice(&bytes[..count]);
            }
            let response = format!(
                "HTTP/1.1 {code} Result\r\nConnection: close\r\nContent-Length: 14\r\n\r\nprivate-secret"
            );
            socket.write_all(response.as_bytes()).unwrap();
            String::from_utf8(request).unwrap()
        });
        let configuration = s3::Configuration::read(&json!({"endpoint":endpoint,
            "region":"us-east-1","access_key":"OLD_ACCESS_ENV_NAME","secret_key":"private-secret"}))
        .unwrap();
        let result = s3::create_bucket(configuration, "uploads".into()).unwrap();
        let request = server.join().unwrap();
        assert!(request.contains("Credential=OLD_ACCESS_ENV_NAME/"));
        assert!(!request.contains("private-secret"));
        assert_eq!(result["http_status"], code);
        assert_eq!(result["status"], expected);
        assert_eq!(result["remote_write_may_have_occurred"], unknown);
        assert_eq!(result["automatic_retry_permitted"], false);
        assert!(!result.to_string().contains("private-secret"));
        assert!(!result.to_string().contains("OLD_ACCESS_ENV_NAME"));
    }
}
