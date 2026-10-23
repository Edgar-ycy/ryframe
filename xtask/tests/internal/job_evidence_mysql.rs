use std::{
    fs,
    io::{self, BufRead, Cursor, Read},
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

use serde_json::{Value, json};

use crate::devex::job_evidence::mysql;

use mysql::{
    invocation::{Environment, command},
    io::{MAX_RESPONSE_BYTES, Response, drain_stderr, query, read_line},
    process_result,
};

struct Fixture {
    _directory: tempfile::TempDir,
    binding: Value,
    environment: Environment,
    executable: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let local = super::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).expect("创建参数测试隔离根目录");
        let directory = tempfile::tempdir_in(local).expect("创建参数测试临时目录");
        let executable = directory
            .path()
            .join(if cfg!(windows) { "mysql.exe" } else { "mysql" });
        // 仅提供格式探针，从不执行此文件。
        let header: &[u8] = if cfg!(windows) {
            b"MZ\0\0"
        } else if cfg!(target_os = "linux") {
            b"\x7fELF"
        } else {
            &[0xfe, 0xed, 0xfa, 0xcf]
        };
        fs::write(&executable, header).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let environment = [
            ("JOB_PASSWORD", "unit-secret-only"),
            ("MYSQL_PWD", "old-unit-password"),
            ("PRESERVED_ENV", "same-value"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
        Self {
            binding: json!({
                "control": {
                    "host": "localhost", "port": "3307", "database": "isolated_control",
                    "username": "test_user", "password": "unit-secret-only", "tls_mode": "required"
                },
                "mysql_client": executable,
                "server_uuid": "baf03c60-671c-47f1-8504-d55640cc108e"
            }),
            _directory: directory,
            environment,
            executable,
        }
    }

    fn legacy_environment(&mut self) {
        for (key, value) in [
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_RUN_ID", "123"),
            ("GITHUB_RUN_ATTEMPT", "2"),
            ("APP_SCOPE_ID", "ci-123-2"),
            ("RYFRAME_CI_MYSQL_CONTAINER_ID", "0123456789abcdef"),
        ] {
            self.environment.insert(key.into(), value.into());
        }
    }
}

fn args(command: &Command) -> Vec<String> {
    command
        .get_args()
        .map(|arg| arg.to_str().unwrap().to_owned())
        .collect()
}

fn accepted_line(bytes: &[u8], response: &mut Response) -> Result<bool, &'static str> {
    response.accept(read_line(&mut Cursor::new(bytes))?.expect("完整测试行"))
}

#[test]
fn direct_invocation_has_fixed_flags_private_file_and_no_password_arguments() {
    let fixture = Fixture::new();
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert_eq!(command.get_program(), fixture.executable.as_os_str());
    let arguments = args(&command);
    assert_eq!(
        arguments[0],
        format!("--defaults-extra-file={}", command.options.path().display())
    );
    assert!(command.options.path().is_absolute());
    assert!(
        command
            .options
            .path()
            .starts_with(super::workspace::root_dir().join(".local-tests"))
    );
    assert_eq!(
        &arguments[1..],
        [
            "--host=127.0.0.1",
            "--port=3307",
            "--user=test_user",
            "--protocol=TCP",
            "--default-character-set=utf8mb4",
            "--ssl-mode=REQUIRED",
            "--batch",
            "--raw",
            "--unbuffered",
            "--skip-column-names",
            "--skip-reconnect",
            "--connect-timeout=5",
        ]
    );
    assert!(!arguments.iter().any(|arg| arg.contains("unit-secret-only")));
    assert!(command.get_envs().all(|(name, _)| name == "SystemRoot"));
    assert_eq!(
        fs::read_to_string(command.options.path()).unwrap(),
        "[client]\npassword=\"unit-secret-only\"\n"
    );
    let path = command.options.path().to_owned();
    let directory = path.parent().unwrap().to_owned();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    // Windows 创建及写入路径在写入前后实际核验 ACL，核验失败不能构造命令。
    drop(command);
    assert!(!path.exists());
    assert!(!directory.exists());
}

#[test]
fn ambient_credentials_ci_switches_and_case_aliases_cannot_change_command() {
    let mut fixture = Fixture::new();
    fixture.legacy_environment();
    for (key, value) in fixture.environment.clone() {
        fixture
            .environment
            .insert(key.to_string_lossy().to_ascii_lowercase().into(), value);
    }
    fixture.environment.insert("APP_ENV".into(), "prod".into());
    fixture
        .environment
        .insert("MYSQL_HOME".into(), "private-location".into());
    fixture
        .environment
        .insert("MYSQL_TEST_LOGIN_FILE".into(), "private-location".into());
    let supplied = fixture.environment.clone();
    let before: Environment = std::env::vars_os().collect();
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert_eq!(command.get_program(), fixture.executable.as_os_str());
    assert!(args(&command).contains(&"--port=3307".into()));
    assert!(command.get_envs().all(|(name, _)| name == "SystemRoot"));
    assert!(supplied == fixture.environment);
    assert!(before == std::env::vars_os().collect());
}

#[test]
fn passwords_are_explicit_and_option_values_escape_special_characters() {
    for (password, encoded) in [
        ("", ""),
        (" spaces # ; = ' \" \\ ", " spaces # ; = ' \\\" \\\\ "),
        ("\n\r\t\u{8}", "\\n\\r\\t\\b"),
        ("中文\\n", "中文\\\\n"),
    ] {
        let mut fixture = Fixture::new();
        fixture.binding["control"]["password"] = json!(password);
        let command = command(&fixture.binding, &Environment::new()).unwrap();
        assert_eq!(
            fs::read_to_string(command.options.path()).unwrap(),
            format!("[client]\npassword=\"{encoded}\"\n")
        );
        assert!(
            !args(&command)
                .iter()
                .any(|arg| arg.starts_with("--password"))
        );
        assert!(!command.get_envs().any(|(key, _)| key == "MYSQL_PWD"));
    }
}

#[test]
fn invalid_password_encoding_and_legacy_fields_fail_without_values() {
    for password in [
        "private\0password",
        "private\u{1b}password",
        "private\u{7f}password",
    ] {
        let mut fixture = Fixture::new();
        fixture.binding["control"]["password"] = json!(password);
        assert!(matches!(
            command(&fixture.binding, &fixture.environment),
            Err("mysql-password-encoding")
        ));
    }
    let mut fixture = Fixture::new();
    fixture.binding["control"]["password_env"] = json!("JOB_PASSWORD");
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("invalid_control_binding")
    ));
    fixture.binding["control"]
        .as_object_mut()
        .unwrap()
        .remove("password");
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("invalid_control_binding")
    ));
}

#[test]
fn spawn_failure_removes_file_and_private_directory_before_returning_error() {
    let fixture = Fixture::new();
    let invocation = command(&fixture.binding, &fixture.environment).unwrap();
    let path = invocation.options.path().to_owned();
    assert!(matches!(invocation.spawn(), Err("mysql-spawn")));
    assert!(!path.exists());
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn option_file_permissions_are_verified_and_changed_access_fails_closed() {
    let mut options = mysql::options::OptionFile::create("unit-secret-only").unwrap();
    options.verify().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(options.path(), fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(windows)]
    {
        // 在空测试 ACL 中加入 Everyone 读取权；产品核验必须拒绝该访问者。
        let executable =
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/icacls.exe");
        let status = Command::new(executable)
            .arg(options.path())
            .args(["/grant", "*S-1-1-0:R"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
    assert_eq!(options.verify(), Err("mysql-option-permissions"));
    options.close().unwrap();
}

#[test]
fn option_file_cleanup_failure_is_reported_and_does_not_delete_unknown_files() {
    let mut options = mysql::options::OptionFile::create("unit-secret-only").unwrap();
    let path = options.path().to_owned();
    let directory = path.parent().unwrap().to_owned();
    let foreign = directory.join("unexpected");
    fs::write(&foreign, b"unrelated").unwrap();
    assert_eq!(options.close(), Err("mysql-option-cleanup"));
    assert!(!path.exists());
    assert!(foreign.is_file());
    fs::remove_file(foreign).unwrap();
    options.close().unwrap();
    assert!(!directory.exists());
}

#[test]
fn host_port_tls_and_argument_injection_fail_closed() {
    for (field, invalid) in [
        ("host", json!("192.168.1.1")),
        ("host", json!("127.0.0.1\n--help")),
        ("port", json!(0)),
        ("port", json!(65536)),
        ("port", json!("3306 --help")),
        ("port", json!(3.5)),
        ("port", json!(true)),
        ("tls_mode", json!("verify_identity")),
        ("tls_mode", json!("required\n")),
        ("username", json!("user\n--help")),
        ("username", json!("user\0name")),
        ("username", json!("--help")),
        ("username", json!("")),
        ("database", json!("schema`")),
        ("database", json!("--help")),
        ("password", json!(false)),
        ("password", json!("private\0value")),
    ] {
        let mut fixture = Fixture::new();
        fixture.binding["control"][field] = invalid;
        assert!(command(&fixture.binding, &fixture.environment).is_err());
    }
}

#[test]
fn missing_or_wrong_typed_connection_fields_fail_before_spawn() {
    let mut fixture = Fixture::new();
    for key in [
        "host", "port", "database", "username", "tls_mode", "password",
    ] {
        let original = fixture.binding["control"]
            .as_object_mut()
            .unwrap()
            .remove(key)
            .unwrap();
        assert!(command(&fixture.binding, &fixture.environment).is_err());
        fixture.binding["control"][key] = Value::Null;
        assert!(command(&fixture.binding, &fixture.environment).is_err());
        fixture.binding["control"][key] = original;
    }
    for binding in [Value::Null, json!([]), json!({"control": false})] {
        assert!(command(&binding, &fixture.environment).is_err());
    }
    fixture.binding["mysql_client"] = json!(false);
    assert!(command(&fixture.binding, &fixture.environment).is_err());
}

#[test]
fn database_and_username_lengths_and_control_characters_are_bounded() {
    for (field, invalid) in [
        ("database", "a".repeat(65)),
        ("database", "".into()),
        ("database", "数据库".into()),
        ("username", "u".repeat(129)),
        ("username", "private\tuser".into()),
        ("username", "private\u{7f}user".into()),
    ] {
        let mut fixture = Fixture::new();
        fixture.binding["control"][field] = json!(invalid);
        assert!(command(&fixture.binding, &fixture.environment).is_err());
    }
}

#[test]
fn valid_port_bounds_and_all_supported_tls_modes() {
    for (port, tls, expected) in [
        (1, "disabled", "DISABLED"),
        (65535, "preferred", "PREFERRED"),
        (3306, "required", "REQUIRED"),
    ] {
        let mut fixture = Fixture::new();
        fixture.binding["control"]["port"] = json!(port);
        fixture.binding["control"]["tls_mode"] = json!(tls);
        let command = command(&fixture.binding, &fixture.environment).unwrap();
        assert!(args(&command).contains(&format!("--ssl-mode={expected}")));
        assert!(args(&command).contains(&format!("--port={port}")));
    }
}

#[test]
fn missing_password_and_invalid_client_files_are_rejected() {
    let mut fixture = Fixture::new();
    fixture.binding["control"]
        .as_object_mut()
        .unwrap()
        .remove("password");
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("invalid_control_binding")
    ));
    fixture.binding["control"]["password"] = json!("unit-secret-only");
    for path in [
        "mysql",
        "mysql.exe",
        "relative/mysql.exe",
        "D:/missing/mysql.exe",
    ] {
        fixture.binding["mysql_client"] = json!(path);
        assert!(command(&fixture.binding, &fixture.environment).is_err());
    }
    fixture.binding["mysql_client"] = json!(fixture.executable);
    fs::write(&fixture.executable, b"#!/bin/sh\necho fake\n").unwrap();
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("mysql-client-binary")
    ));
    fs::remove_file(&fixture.executable).unwrap();
    fs::create_dir(&fixture.executable).unwrap();
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("mysql-client-path")
    ));
}

#[test]
fn client_parent_traversal_and_script_contents_are_rejected() {
    let mut fixture = Fixture::new();
    fixture.binding["mysql_client"] = json!(
        fixture
            ._directory
            .path()
            .join("child")
            .join("..")
            .join(fixture.executable.file_name().unwrap())
    );
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("mysql-client-path")
    ));
    for name in [
        "mysql.cmd",
        "mysql.bat",
        "mysql.ps1",
        "mysql.sh",
        "other.exe",
    ] {
        let path = fixture._directory.path().join(name);
        fs::copy(&fixture.executable, &path).unwrap();
        fs::write(&path, b"#!/bin/sh\necho fake\n").unwrap();
        fixture.binding["mysql_client"] = json!(path);
        assert!(matches!(
            command(&fixture.binding, &fixture.environment),
            Err("mysql-client-binary")
        ));
    }
}

#[test]
fn explicitly_renamed_client_binary_is_accepted() {
    let mut fixture = Fixture::new();
    let renamed = fixture._directory.path().join(if cfg!(windows) {
        "isolated-client.exe"
    } else {
        "isolated-client"
    });
    fs::rename(&fixture.executable, &renamed).unwrap();
    fixture.binding["mysql_client"] = json!(renamed);
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert_eq!(command.get_program(), renamed.as_os_str());
}

#[cfg(unix)]
#[test]
fn non_executable_client_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.executable, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("mysql-client-path")
    ));
}

#[cfg(unix)]
#[test]
fn linked_client_and_linked_parent_directory_are_rejected() {
    use std::os::unix::fs::symlink;
    let mut fixture = Fixture::new();
    let other = fixture._directory.path().join("other");
    fs::rename(&fixture.executable, &other).unwrap();
    symlink(&other, &fixture.executable).unwrap();
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("mysql-client-path")
    ));
    fs::remove_file(&fixture.executable).unwrap();
    fs::rename(&other, &fixture.executable).unwrap();
    let linked = fixture._directory.path().join("linked");
    symlink(fixture._directory.path(), &linked).unwrap();
    fixture.binding["mysql_client"] = json!(linked.join("mysql"));
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("mysql-client-path")
    ));
}

#[test]
fn bounded_lines_preserve_exact_json_bytes_and_handle_crlf_and_eof() {
    let mut reader = Cursor::new(b"{\"name\":\"\xe4\xb8\xad\xe6\x96\x87\"}\r\n{}\n");
    let first = read_line(&mut reader).unwrap().unwrap();
    assert_eq!(first.bytes, "{\"name\":\"中文\"}".as_bytes());
    assert_eq!(first.wire_bytes, first.bytes.len() + 2);
    assert_eq!(read_line(&mut reader).unwrap().unwrap().bytes, b"{}");
    assert!(read_line(&mut reader).unwrap().is_none());
    assert!(matches!(
        read_line(&mut Cursor::new(b"{}")),
        Err("mysql-output-incomplete")
    ));
}

#[test]
fn single_line_limit_is_bounded_even_without_newline() {
    let mut exact = vec![b' '; MAX_RESPONSE_BYTES];
    exact[MAX_RESPONSE_BYTES - 1] = b'\n';
    assert_eq!(
        read_line(&mut Cursor::new(exact))
            .unwrap()
            .unwrap()
            .wire_bytes,
        MAX_RESPONSE_BYTES
    );
    let oversized = vec![b'x'; MAX_RESPONSE_BYTES + 8192];
    let mut reader = Cursor::new(oversized);
    assert!(matches!(read_line(&mut reader), Err("mysql-output-limit")));
    assert_eq!(reader.position(), (MAX_RESPONSE_BYTES + 1) as u64);
}

#[test]
fn per_query_limit_includes_all_rows_and_sentinel_line() {
    let mut response = Response::new("gate-test".into());
    let large = format!("\"{}\"\n", "x".repeat(MAX_RESPONSE_BYTES - 6));
    assert!(!accepted_line(large.as_bytes(), &mut response).unwrap());
    assert!(matches!(
        accepted_line(b"gate-test\n", &mut response),
        Err("mysql-output-limit")
    ));
}

#[test]
fn response_accepts_exact_total_byte_limit_including_sentinel() {
    let sentinel = "gate-test";
    let mut response = Response::new(sentinel.into());
    let large = format!(
        "\"{}\"\n",
        "x".repeat(MAX_RESPONSE_BYTES - sentinel.len() - 4)
    );
    assert!(!accepted_line(large.as_bytes(), &mut response).unwrap());
    assert!(accepted_line(format!("{sentinel}\n").as_bytes(), &mut response).unwrap());
    assert_eq!(response.finish().len(), 1);
}

#[test]
fn bounded_read_does_not_consume_the_next_response_line() {
    let mut reader = Cursor::new(b"{}\ngate-test\n");
    let mut response = Response::new("gate-test".into());
    assert!(
        !response
            .accept(read_line(&mut reader).unwrap().unwrap())
            .unwrap()
    );
    assert_eq!(reader.position(), 3);
    assert!(
        response
            .accept(read_line(&mut reader).unwrap().unwrap())
            .unwrap()
    );
    assert!(read_line(&mut reader).unwrap().is_none());
}

#[test]
fn sentinel_is_unique_per_query_and_only_complete_matching_line_ends_response() {
    let (first, sentinel) = query("SELECT JSON_OBJECT('id','1');\n").unwrap();
    let (_, second_sentinel) = query("SELECT JSON_OBJECT('id','1')").unwrap();
    assert_ne!(sentinel, second_sentinel);
    assert_eq!(sentinel.len(), 37);
    assert!(
        std::str::from_utf8(&first)
            .unwrap()
            .ends_with(&format!(";\nSELECT '{sentinel}';\n"))
    );
    let mut response = Response::new(sentinel.clone());
    assert!(!accepted_line(b"{\"id\":\"1\"}\n", &mut response).unwrap());
    assert!(!accepted_line(format!("\"{sentinel}\"\n").as_bytes(), &mut response).unwrap());
    assert!(accepted_line(format!("{sentinel}\n").as_bytes(), &mut response).unwrap());
    assert_eq!(response.finish(), vec![json!({"id": "1"}), json!(sentinel)]);
}

#[test]
fn output_rejects_duplicate_keys_invalid_utf8_nonfinite_and_non_json() {
    for bytes in [
        b"{\"x\":1,\"x\":2}\n".as_slice(),
        b"{\"x\":{\"y\":1,\"y\":2}}\n",
        b"{\"a\":1,\"\\u0061\":2}\n",
        b"{\"x\":NaN}\n",
        b"{\"x\":Infinity}\n",
        b"{\"x\":1e999}\n",
        b"{\"x\":\"\xff\"}\n",
        b"ERROR raw-private-stderr\n",
        b"[{\"x\":1,\"x\":2}]\n",
        b"{} {}\n",
        b"\n",
    ] {
        let mut response = Response::new("gate-test".into());
        let error = accepted_line(bytes, &mut response).unwrap_err();
        assert!(matches!(error, "mysql-output-json" | "mysql-output-utf8"));
        assert!(!error.contains("raw-private-stderr"));
    }
}

#[test]
fn malformed_or_unbounded_query_input_is_rejected_without_outputting_sql() {
    for sql in ["", ";\n", " \t\r\n", "SELECT\0private-sql"] {
        assert!(matches!(query(sql), Err("mysql-query")));
    }
    assert!(matches!(
        query(&"x".repeat(MAX_RESPONSE_BYTES + 1)),
        Err("mysql-query")
    ));
}

struct BrokenReader;

impl Read for BrokenReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("private-io-diagnostic"))
    }
}

impl BufRead for BrokenReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        Err(io::Error::other("private-io-diagnostic"))
    }

    fn consume(&mut self, _: usize) {}
}

#[test]
fn read_error_is_fixed_category_without_raw_diagnostics() {
    assert!(matches!(
        read_line(&mut BrokenReader),
        Err("mysql-stdout-read")
    ));
}

struct InterruptedReader {
    interrupted: bool,
    bytes: Cursor<&'static [u8]>,
}

impl Read for InterruptedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        self.bytes.read(buffer)
    }
}

#[test]
fn stderr_read_retries_interruptions_and_reaches_eof() {
    let failure = Arc::new(AtomicU8::new(0));
    let mut reader = InterruptedReader {
        interrupted: false,
        bytes: Cursor::new(b"private diagnostic"),
    };
    drain_stderr(&mut reader, Arc::clone(&failure));
    assert_eq!(reader.bytes.position(), b"private diagnostic".len() as u64);
    assert_eq!(failure.load(Ordering::Acquire), 1);
}

#[test]
fn stderr_is_drained_without_retaining_private_diagnostics() {
    let failure = Arc::new(AtomicU8::new(0));
    let bytes = vec![b'x'; MAX_RESPONSE_BYTES + 4096];
    let mut reader = Cursor::new(bytes);
    drain_stderr(&mut reader, Arc::clone(&failure));
    assert_eq!(reader.position(), (MAX_RESPONSE_BYTES + 4096) as u64);
    assert_eq!(failure.load(Ordering::Acquire), 1);

    let failure = Arc::new(AtomicU8::new(0));
    drain_stderr(Cursor::new([]), Arc::clone(&failure));
    assert_eq!(failure.load(Ordering::Acquire), 0);
    drain_stderr(BrokenReader, Arc::clone(&failure));
    assert_eq!(failure.load(Ordering::Acquire), 2);
}

#[test]
fn warning_stderr_with_valid_sentinel_and_successful_process_result_is_accepted() {
    let failure = Arc::new(AtomicU8::new(0));
    drain_stderr(
        Cursor::new(b"Warning: private diagnostic\n"),
        Arc::clone(&failure),
    );
    let mut response = Response::new("gate-test".into());
    assert!(!accepted_line(b"{\"id\":\"1\"}\n", &mut response).unwrap());
    assert!(accepted_line(b"gate-test\n", &mut response).unwrap());
    assert_eq!(response.finish(), vec![json!({"id": "1"})]);
    // check_running 的健康结果和 close 的正常退出均经由同一分类入口。
    assert_eq!(
        process_result(Ok(()), failure.load(Ordering::Acquire)),
        Ok(())
    );
}

#[test]
fn stderr_read_failure_rejects_otherwise_valid_response_and_normal_exit() {
    let failure = Arc::new(AtomicU8::new(0));
    drain_stderr(BrokenReader, Arc::clone(&failure));
    let mut response = Response::new("gate-test".into());
    assert!(accepted_line(b"gate-test\n", &mut response).unwrap());
    assert_eq!(
        process_result(Ok(()), failure.load(Ordering::Acquire)),
        Err("mysql-stderr-read")
    );
}

#[test]
fn warning_stderr_does_not_mask_process_eof_or_missing_sentinel_failures() {
    for error in [
        "mysql-process-exit",
        "mysql-close-exit",
        "mysql-eof",
        "mysql-timeout",
    ] {
        assert_eq!(process_result(Err(error), 1), Err(error));
    }
    let mut response = Response::new("gate-test".into());
    assert!(!accepted_line(b"{\"id\":\"1\"}\n", &mut response).unwrap());
    assert!(read_line(&mut Cursor::new([])).unwrap().is_none());
}
