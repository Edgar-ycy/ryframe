use super::*;
use std::{path::PathBuf, sync::OnceLock};

fn directory() -> tempfile::TempDir {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    fs::create_dir(directory.path().join(".local-tests")).unwrap();
    directory
}

const REMOTE_STORAGE: &str = "[object_storage]\nbackend='s3'\nendpoint='http://127.0.0.1:9000'\n\
    region='us-east-1'\naccess_key='access-fixture'\nsecret_key='secret-fixture'\n";

fn frozen_storage(storage: impl FnOnce(&Path) -> String) -> (tempfile::TempDir, Context, Frozen) {
    let root = directory();
    let context = Context {
        root: root.path().into(),
        output: root.path().join("runtime"),
        control: Value::Null,
        run: "42".into(),
        attempt: "3".into(),
    };
    fs::create_dir(&context.output).unwrap();
    let directory = context.root.join(".local-tests/full-stack-config");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("app.toml"), "# common=A\r\n").unwrap();
    let mut text = "scope_id='ci-42-3'\nsnowflake_worker_id=3\n\
        [database.primary]\nhost='127.0.0.1'\nport=3306\ndatabase='ryframe_e2e_42_3'\n\
        username='root'\npassword='db-fixture-secret'\ntls_mode='required'\n"
        .to_owned();
    text.push_str(&storage(&context.root));
    fs::write(directory.join("app.test.toml"), text).unwrap();
    let frozen =
        Frozen::capture(&context.root, &context.output, "42", "3", &"a".repeat(64)).unwrap();
    (root, context, frozen)
}

#[test]
fn native_storage_rejects_same_size_configuration_changes_before_probing() {
    for (name, before, after) in [
        ("app.toml", "common=A", "common=B"),
        ("app.test.toml", "secret-fixture", "secret-changed"),
    ] {
        let (_root, context, frozen) = frozen_storage(|_| {
            REMOTE_STORAGE.replace("http://127.0.0.1:9000", "invalid-endpoint-before-probe")
        });
        frozen.object_configuration_binding().unwrap();
        let path = context
            .root
            .join(".local-tests/full-stack-config")
            .join(name);
        let original = fs::read_to_string(&path).unwrap();
        let changed = original.replace(before, after);
        assert_ne!(original, changed);
        assert_eq!(original.len(), changed.len());
        fs::write(path, changed).unwrap();
        let mut native = Native {
            context: &context,
            cancel: Arc::new(AtomicBool::new(false)),
            api: None,
        };
        let error = native.storage(&frozen).unwrap_err().to_string();
        assert!(error.contains("未启动对象准备"));
        assert!(!error.contains("secret-fixture") && !error.contains("secret-changed"));
        assert_eq!(fs::read_dir(&context.output).unwrap().count(), 0);
    }
}

#[test]
fn native_storage_sends_bound_create_and_head_for_each_bucket() {
    let (_root, context, frozen) = frozen_storage(|_| REMOTE_STORAGE.into());
    let scope = frozen.scope().unwrap();
    let binding = frozen.object_configuration_binding().unwrap();
    let configuration =
        crate::recovery::clone::native::s3::app_configuration::private_configuration(
            &context.root,
            &binding,
        )
        .unwrap();
    assert!(configuration["access_key"] == "access-fixture");
    let path = context.output.join("ci-storage.json");
    let mut calls = Vec::new();
    storage::prepare_buckets(&context.output, &scope, &binding, |marker, request| {
        let state = documents::read(&path)?;
        assert_eq!(state["automatic_retry_permitted"], false);
        assert_eq!(
            state["buckets"].as_array().unwrap().last().unwrap(),
            &json!({"bucket":request["bucket"],"state":"creation-unknown"})
        );
        let encoded = request.to_string();
        assert!(!encoded.contains("secret-fixture") && !encoded.contains("access-fixture"));
        calls.push((marker.to_owned(), request.clone()));
        Ok(json!({"status":"created_unverified"}))
    })
    .unwrap();
    assert_eq!(calls.len(), 10);
    for (bucket, pair) in ["uploads", "avatar", "exports", "imports", "config-packages"]
        .into_iter()
        .zip(calls.as_chunks::<2>().0)
    {
        assert_eq!(
            pair[0],
            (
                "__s3-create-bucket".into(),
                json!({
                    "format_version":1,"kind":"ryframe-native-s3-create-bucket","scope_id":scope,
                    "configuration_binding":binding,"bucket":bucket
                })
            )
        );
        assert_eq!(
            pair[1],
            (
                "__s3-inspect".into(),
                json!({
                    "format_version":1,"kind":"ryframe-native-s3-inspect","scope_id":scope,
                    "configuration_binding":binding,"bucket":bucket,"operation":"head-bucket",
                    "prefix":null,"continuation":null
                })
            )
        );
    }
    let receipt = documents::read(&path).unwrap();
    assert_eq!(receipt["buckets"].as_array().unwrap().len(), 5);
    assert!(
        receipt["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|bucket| bucket["state"] == "created")
    );
}

#[test]
fn native_storage_retains_unknown_bucket_and_does_not_send_again() {
    let (_root, context, frozen) = frozen_storage(|_| REMOTE_STORAGE.into());
    let scope = frozen.scope().unwrap();
    let binding = frozen.object_configuration_binding().unwrap();
    let mut calls = Vec::new();
    let result = storage::prepare_buckets(&context.output, &scope, &binding, |marker, _| {
        calls.push(marker.to_owned());
        if marker == "__s3-inspect" {
            return Err("HEAD 失响".into());
        }
        Ok(json!({"status":"created_unverified"}))
    });
    assert!(result.unwrap_err().to_string().contains("HEAD 失响"));
    let path = context.output.join("ci-storage.json");
    let original = fs::read(&path).unwrap();
    let receipt = documents::read(&path).unwrap();
    assert_eq!(
        receipt["buckets"],
        json!([{"bucket":"uploads","state":"creation-unknown"}])
    );
    assert_eq!(receipt["automatic_retry_permitted"], false);
    assert!(
        storage::prepare_buckets(&context.output, &scope, &binding, |marker, _| {
            calls.push(marker.to_owned());
            Ok(json!({}))
        })
        .is_err()
    );
    assert_eq!(calls, ["__s3-create-bucket", "__s3-inspect"]);
    assert_eq!(fs::read(path).unwrap(), original);
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
fn native_registration_rejects_partial_and_non_file_receipts_without_writing() {
    for name in ["runtime.json", "runtime-evidence.json"] {
        for is_directory in [false, true] {
            let root = directory();
            let context = Context {
                root: root.path().into(),
                output: root.path().into(),
                control: Value::Null,
                run: String::new(),
                attempt: String::new(),
            };
            let mut native = Native {
                context: &context,
                cancel: Arc::new(AtomicBool::new(false)),
                api: None,
            };
            let path = context.output.join(name);
            if is_directory {
                fs::create_dir(&path).unwrap();
            } else {
                fs::write(&path, b"preserve original").unwrap();
            }

            let error = native.leaf("register").unwrap_err().to_string();
            if is_directory {
                assert!(error.contains("普通文件"));
                assert!(path.is_dir());
            } else {
                assert!(error.contains("只存在一份收据"));
                assert_eq!(fs::read(&path).unwrap(), b"preserve original");
            }
            assert_eq!(fs::read_dir(&context.output).unwrap().count(), 2);
        }
    }
}

#[cfg(unix)]
#[test]
fn native_registration_preserves_dangling_receipt_links() {
    let root = directory();
    let context = Context {
        root: root.path().into(),
        output: root.path().into(),
        control: Value::Null,
        run: String::new(),
        attempt: String::new(),
    };
    let mut native = Native {
        context: &context,
        cancel: Arc::new(AtomicBool::new(false)),
        api: None,
    };
    let receipt = context.output.join("runtime-evidence.json");
    let missing = context.output.join("absent");
    std::os::unix::fs::symlink(&missing, &receipt).unwrap();

    let error = native.leaf("register").unwrap_err().to_string();
    assert!(error.contains("符号链接"));
    assert_eq!(fs::read_link(receipt).unwrap(), missing);
    assert_eq!(fs::read_dir(&context.output).unwrap().count(), 2);
}

#[test]
fn capture_preserves_stdin_already_bound_to_the_role_command() {
    let root = directory();
    let mut command = command("echo", root.path());
    let input = root.path().join("bound-input");
    fs::write(&input, b"bound role request").unwrap();
    command.stdin(fs::File::open(input).unwrap());
    let output = capture_prebound(
        command,
        root.path(),
        Duration::from_secs(5),
        None,
        true,
        CaptureOutput::Separate,
    )
    .unwrap();
    assert_eq!(output, "bound role request");
    gone(root.path());
}

const API_RUNTIME_REJECTION: &str =
    "API运行合同核验失败；核对冻结来源、配置与产物，保留现场且不启动或停止进程";

fn api_rejection(message: &str) -> String {
    json!({"ok":false,"error":"ProcessRejected","message":message}).to_string()
}

#[test]
fn api_control_diagnostics_recognize_only_complete_safe_responses() {
    let preparation = format!(
        "API监督准备失败；阶段=member_spawn；原因=permission_denied；系统码=13；原因摘要={}；核对成员创建权限与系统资源；保留现场，显式核对，不重放",
        "a".repeat(64)
    );
    for message in [API_RUNTIME_REJECTION, preparation.as_str()] {
        let response = api_rejection(message);
        assert_eq!(
            worker::api_control_diagnostic(response.as_bytes()).as_deref(),
            Some(message)
        );
        let changed = api_rejection(&format!("{message} private-password"));
        assert!(worker::api_control_diagnostic(changed.as_bytes()).is_none());
    }
    let response = api_rejection(API_RUNTIME_REJECTION);
    for invalid in [
        api_rejection("API rejected with private-password"),
        response.replace("ProcessRejected", "WorkerRejected"),
        response.replace("false", "0"),
        r#"{"ok":false,"error":"ProcessRejected","message":13}"#.to_owned(),
        format!("{{\"extra\":true,{}", &response[1..]),
        format!("{{\"message\":\"private-password\",{}", &response[1..]),
        format!("{response} null"),
    ] {
        assert!(worker::api_control_diagnostic(invalid.as_bytes()).is_none());
    }
    assert!(worker::api_control_diagnostic(&[0xff]).is_none());
    let mut oversized = response.into_bytes();
    oversized.resize(1024 * 1024 + 1, b' ');
    assert!(worker::api_control_diagnostic(&oversized).is_none());
}

#[test]
fn capture_exposes_safe_diagnostics_only_from_failed_api_control() {
    let root = directory();
    let input = root.path().join("rejected-api-response");
    let private = api_rejection(&format!("{API_RUNTIME_REJECTION} private-password"));
    let safe = api_rejection(API_RUNTIME_REJECTION);
    for (output_mode, response, expected) in [
        (CaptureOutput::ApiControl, &safe, true),
        (CaptureOutput::Separate, &safe, false),
        (CaptureOutput::ApiControl, &private, false),
    ] {
        fs::write(&input, response).unwrap();
        let mut command = command("echo-fail", root.path());
        command.stdin(fs::File::open(&input).unwrap());
        let error = capture_prebound(
            command,
            root.path(),
            Duration::from_secs(5),
            None,
            true,
            output_mode,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("退出状态") && error.contains("17"));
        assert_eq!(error.contains(API_RUNTIME_REJECTION), expected);
        assert!(!error.contains("private-password"));
    }
    gone(root.path());
}

#[test]
fn native_api_absence_and_cancelled_start_are_read_only() {
    let root = directory();
    let context = Context {
        root: root.path().into(),
        output: root.path().into(),
        control: Value::Null,
        run: String::new(),
        attempt: String::new(),
    };
    let mut native = Native {
        context: &context,
        cancel: Arc::new(AtomicBool::new(true)),
        api: None,
    };
    assert_eq!(native.leaf("api-stop").unwrap()["state"], "absent");
    assert!(
        native
            .leaf("api-launch")
            .unwrap_err()
            .to_string()
            .contains("取消")
    );
    assert_eq!(fs::read_dir(&context.output).unwrap().count(), 1);
}

#[test]
fn native_api_stop_preserves_unowned_members_and_unknown_receipts() {
    for name in [
        "api-members-00000000000000000000000000000001-ready.json",
        "api-unknown.json",
        "api-tree.json",
        "api-control.lock",
    ] {
        let root = directory();
        let context = Context {
            root: root.path().into(),
            output: root.path().into(),
            control: Value::Null,
            run: String::new(),
            attempt: String::new(),
        };
        let mut native = Native {
            context: &context,
            cancel: Arc::new(AtomicBool::new(false)),
            api: None,
        };
        let path = context.output.join(name);
        fs::write(&path, b"original").unwrap();
        assert!(native.leaf("api-stop").is_err(), "{name}");
        assert_eq!(fs::read(path).unwrap(), b"original");
        assert_eq!(fs::read_dir(&context.output).unwrap().count(), 2);
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
fn native_pnpm_failure_logs_only_known_codes_with_exit_cwd_and_no_replay() {
    let root = directory();
    let context = Context {
        root: root.path().into(),
        output: root.path().into(),
        control: Value::Null,
        run: String::new(),
        attempt: String::new(),
    };
    let mut native = Native {
        context: &context,
        cancel: Arc::new(AtomicBool::new(false)),
        api: None,
    };
    for (mode, offline, fetch) in [
        ("pnpm-offline-fail", true, true),
        ("pnpm-fetch-fail", false, true),
        ("pnpm-unknown-fail", false, false),
    ] {
        let log = root.path().join(format!("{mode}.log"));
        let error = native
            .command_at(
                root.path(),
                fixture().to_str().unwrap(),
                &[mode.into(), root.path().to_str().unwrap().into()],
                Some(&log),
            )
            .unwrap_err()
            .to_string();
        let logged = fs::read_to_string(log).unwrap();
        assert!(error.contains("17"));
        assert!(error.contains(root.path().to_str().unwrap()));
        assert_eq!(logged.trim(), error);
        assert_eq!(error.contains("ERR_PNPM_NO_OFFLINE_META"), offline);
        assert_eq!(error.contains("ERR_PNPM_META_FETCH_FAIL"), fetch);
        assert_eq!(
            error.contains("verification.pnpm_cache_dir"),
            offline || fetch
        );
        if offline {
            assert!(error.contains("所需缓存/元数据和冻结依赖"));
            assert!(error.contains("新输出目录"));
        }
        assert!(!error.contains("private-password"));
        assert!(!error.contains("example.invalid"));
        assert!(!error.contains("ERR_PNPM_UNKNOWN"));
    }
    assert_eq!(
        fs::read_to_string(root.path().join("pnpm-invocations")).unwrap(),
        "pnpm-offline-fail\npnpm-fetch-fail\npnpm-unknown-fail\n"
    );
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
    let (_root, context, frozen) = frozen_storage(|root| {
        format!(
            "[object_storage]\nbackend='local'\nlocal_base_dir={}\nendpoint='invalid-unused-endpoint'\n",
            json!(root.join("绑定配置 本地对象"))
        )
    });
    fs::remove_file(context.root.join(".local-tests/full-stack-config/app.toml")).unwrap();
    let mut native = Native {
        context: &context,
        cancel: Arc::new(AtomicBool::new(false)),
        api: None,
    };
    let storage = context.root.join("绑定配置 本地对象");
    native.storage(&frozen).unwrap();
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
    native.storage(&frozen).unwrap();
    assert_eq!(fs::read(existing).unwrap(), b"keep");
    assert!(!context.output.join("ci-storage.json").exists());
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
            socket.set_nonblocking(false).unwrap();
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
