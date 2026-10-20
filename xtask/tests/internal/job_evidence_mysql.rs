use std::{
    ffi::OsStr,
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
                    "username": "test_user", "password_env": "JOB_PASSWORD", "tls_mode": "required"
                },
                "mysql_client": executable,
                "server_uuid": "baf03c60-671c-47f1-8504-d55640cc108e"
            }),
            _directory: directory,
            environment,
            executable,
        }
    }

    fn docker(&mut self) {
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
fn direct_invocation_has_fixed_mysql_flags_and_no_password_arguments() {
    let fixture = Fixture::new();
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert_eq!(command.get_program(), fixture.executable.as_os_str());
    assert_eq!(
        args(&command),
        [
            "--no-defaults",
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
    assert!(
        !args(&command)
            .iter()
            .any(|arg| arg.contains("unit-secret-only"))
    );
    assert!(command.get_envs().any(|(key, value)| {
        key == "MYSQL_PWD" && value == Some(OsStr::new("unit-secret-only"))
    }));
}

#[test]
fn command_build_does_not_mutate_process_or_supplied_environment() {
    let fixture = Fixture::new();
    let supplied = fixture.environment.clone();
    let before: Environment = std::env::vars_os().collect();
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert!(before == std::env::vars_os().collect());
    assert!(supplied == fixture.environment);
    assert!(
        command.get_envs().any(|(key, value)| {
            key == "PRESERVED_ENV" && value == Some(OsStr::new("same-value"))
        })
    );
    assert!(
        !command
            .get_envs()
            .any(|(key, value)| key == "JOB_PASSWORD" && value.is_some())
    );
}

#[cfg(windows)]
#[test]
fn windows_password_lookup_accepts_mixed_case_environment_key() {
    let mut fixture = Fixture::new();
    let password = fixture
        .environment
        .remove(OsStr::new("JOB_PASSWORD"))
        .unwrap();
    fixture.environment.insert("jOb_pAsSwOrD".into(), password);
    let supplied = fixture.environment.clone();
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert!(supplied == fixture.environment);
    assert!(command.get_envs().any(|(key, value)| {
        key.as_encoded_bytes().eq_ignore_ascii_case(b"MYSQL_PWD")
            && value == Some(OsStr::new("unit-secret-only"))
    }));
    assert!(!command.get_envs().any(|(key, value)| {
        key.as_encoded_bytes().eq_ignore_ascii_case(b"JOB_PASSWORD") && value.is_some()
    }));
}

#[cfg(windows)]
#[test]
fn windows_ci_lookup_accepts_mixed_case_environment_keys() {
    let mut fixture = Fixture::new();
    fixture.docker();
    for (canonical, mixed) in [
        ("GITHUB_ACTIONS", "github_Actions"),
        ("GITHUB_RUN_ID", "Github_Run_Id"),
        ("GITHUB_RUN_ATTEMPT", "github_run_Attempt"),
        ("APP_SCOPE_ID", "app_Scope_Id"),
        (
            "RYFRAME_CI_MYSQL_CONTAINER_ID",
            "ryframe_CI_mysql_Container_Id",
        ),
    ] {
        let value = fixture.environment.remove(OsStr::new(canonical)).unwrap();
        fixture.environment.insert(mixed.into(), value);
    }
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert_eq!(command.get_program(), OsStr::new("docker"));
    assert_eq!(
        &args(&command)[..6],
        [
            "exec",
            "-i",
            "--env",
            "MYSQL_PWD",
            "0123456789abcdef",
            "mysql"
        ]
    );
    assert!(args(&command).contains(&"--port=3306".into()));
}

#[cfg(windows)]
#[test]
fn windows_known_environment_keys_reject_duplicate_case_aliases_without_values() {
    for key in [
        "JOB_PASSWORD",
        "GITHUB_ACTIONS",
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
        "APP_SCOPE_ID",
        "RYFRAME_CI_MYSQL_CONTAINER_ID",
    ] {
        for same_value in [false, true] {
            let mut fixture = Fixture::new();
            fixture.docker();
            let value = if same_value {
                fixture.environment.get(OsStr::new(key)).unwrap().clone()
            } else {
                "private-conflicting-value".into()
            };
            fixture
                .environment
                .insert(key.to_ascii_lowercase().into(), value);
            assert!(matches!(
                command(&fixture.binding, &fixture.environment),
                Err("mysql-environment-ambiguous")
            ));
        }
    }
}

#[cfg(unix)]
#[test]
fn unix_password_lookup_keeps_exact_case_and_distinct_aliases() {
    let mut fixture = Fixture::new();
    let password = fixture
        .environment
        .remove(OsStr::new("JOB_PASSWORD"))
        .unwrap();
    fixture.environment.insert("job_password".into(), password);
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("mysql-password-missing")
    ));
    fixture
        .environment
        .insert("JOB_PASSWORD".into(), "exact-case-secret".into());
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert!(command.get_envs().any(|(key, value)| {
        key == "MYSQL_PWD" && value == Some(OsStr::new("exact-case-secret"))
    }));
}

#[cfg(unix)]
#[test]
fn unix_ci_switch_does_not_match_lowercase_environment_key() {
    let mut fixture = Fixture::new();
    fixture.docker();
    let value = fixture
        .environment
        .remove(OsStr::new("GITHUB_ACTIONS"))
        .unwrap();
    fixture.environment.insert("github_actions".into(), value);
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert_eq!(command.get_program(), fixture.executable.as_os_str());
}

#[cfg(unix)]
#[test]
fn unix_ci_owner_lookup_requires_exact_keys_and_allows_distinct_aliases() {
    for key in [
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
        "APP_SCOPE_ID",
        "RYFRAME_CI_MYSQL_CONTAINER_ID",
    ] {
        let mut fixture = Fixture::new();
        fixture.docker();
        let value = fixture.environment.remove(OsStr::new(key)).unwrap();
        fixture
            .environment
            .insert(key.to_ascii_lowercase().into(), value.clone());
        assert!(matches!(
            command(&fixture.binding, &fixture.environment),
            Err("mysql-ci-owner")
        ));
        fixture.environment.insert(key.into(), value);
        let command = command(&fixture.binding, &fixture.environment).unwrap();
        assert_eq!(command.get_program(), OsStr::new("docker"));
    }
}

#[test]
fn password_can_be_empty_or_already_named_mysql_pwd_without_changing_arguments() {
    for password in ["", "private password with spaces = and quotes '\""] {
        let mut fixture = Fixture::new();
        fixture.binding["control"]["password_env"] = json!("MYSQL_PWD");
        fixture
            .environment
            .insert("MYSQL_PWD".into(), password.into());
        let command = command(&fixture.binding, &fixture.environment).unwrap();
        assert!(
            command
                .get_envs()
                .any(|(key, value)| { key == "MYSQL_PWD" && value == Some(OsStr::new(password)) })
        );
        assert!(
            !args(&command)
                .iter()
                .any(|arg| arg.starts_with("--password"))
        );
    }
}

#[test]
fn password_with_nul_is_rejected_without_exposing_its_value() {
    let mut fixture = Fixture::new();
    fixture
        .environment
        .insert("JOB_PASSWORD".into(), "private\0password".into());
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("mysql-password-env")
    ));
}

#[test]
fn ci_uses_owned_container_stdin_env_reference_and_container_port() {
    let mut fixture = Fixture::new();
    fixture.docker();
    fixture.binding["mysql_client"] = json!("not-used-in-ci");
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert_eq!(command.get_program(), OsStr::new("docker"));
    let args = args(&command);
    assert_eq!(
        &args[..6],
        [
            "exec",
            "-i",
            "--env",
            "MYSQL_PWD",
            "0123456789abcdef",
            "mysql"
        ]
    );
    assert!(args.iter().any(|arg| arg == "--port=3306"));
    assert!(!args.iter().any(|arg| arg.contains("unit-secret-only")));
}

#[test]
fn ci_rejects_invalid_container_run_attempt_and_scope() {
    for (key, invalid) in [
        ("RYFRAME_CI_MYSQL_CONTAINER_ID", "0123456789a"),
        ("RYFRAME_CI_MYSQL_CONTAINER_ID", "0123456789ABCDEF"),
        ("RYFRAME_CI_MYSQL_CONTAINER_ID", "--privileged"),
        ("GITHUB_RUN_ID", "0"),
        ("GITHUB_RUN_ID", "0123"),
        ("GITHUB_RUN_ID", "123\n"),
        ("GITHUB_RUN_ATTEMPT", "-2"),
        ("APP_SCOPE_ID", "ci-123-1"),
    ] {
        let mut fixture = Fixture::new();
        fixture.docker();
        fixture.environment.insert(key.into(), invalid.into());
        assert!(matches!(
            command(&fixture.binding, &fixture.environment),
            Err("mysql-ci-owner")
        ));
    }
    let mut fixture = Fixture::new();
    fixture.docker();
    fixture.environment.insert(
        "RYFRAME_CI_MYSQL_CONTAINER_ID".into(),
        "a".repeat(65).into(),
    );
    assert!(command(&fixture.binding, &fixture.environment).is_err());
}

#[test]
fn ci_requires_each_ownership_environment_field() {
    for key in [
        "RYFRAME_CI_MYSQL_CONTAINER_ID",
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
        "APP_SCOPE_ID",
    ] {
        let mut fixture = Fixture::new();
        fixture.docker();
        fixture.environment.remove(OsStr::new(key));
        assert!(matches!(
            command(&fixture.binding, &fixture.environment),
            Err("mysql-ci-owner")
        ));
    }
}

#[test]
fn only_exact_github_actions_true_enables_docker() {
    let mut fixture = Fixture::new();
    fixture
        .environment
        .insert("GITHUB_ACTIONS".into(), "TRUE".into());
    let command = command(&fixture.binding, &fixture.environment).unwrap();
    assert_eq!(command.get_program(), fixture.executable.as_os_str());
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
        ("password_env", json!("JOB_PASSWORD=other")),
        ("password_env", json!("bad_name")),
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
        "host",
        "port",
        "database",
        "username",
        "tls_mode",
        "password_env",
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
    fixture.environment.remove(OsStr::new("JOB_PASSWORD"));
    assert!(matches!(
        command(&fixture.binding, &fixture.environment),
        Err("mysql-password-missing")
    ));
    fixture
        .environment
        .insert("JOB_PASSWORD".into(), "unit-secret-only".into());
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
