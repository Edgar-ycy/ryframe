//! 核验授权边界、工具后继和unknown处理；不以这些用例替代真实Redis生命周期。
use super::start_deadline::Deadline;
use super::{
    super::digest,
    diagnostic::{Category, Diagnostic, Outcome},
    host_control, model, native_tool, process,
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[test]
fn formal_cache_status_is_read_only_and_direct_guard_failure_never_connects_or_publishes() {
    use super::{runner, runtime::Native, session::Runtime};
    use crate::{
        cli::{CacheOperation, CacheOptions},
        recovery::clone::native::{json as document, lifecycle::state, resources::common},
    };
    use std::{fs, net::TcpListener};
    let backend = crate::workspace::root_dir();
    fs::create_dir_all(backend.join(".local-tests")).unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("cache-direct-")
        .tempdir_in(backend.join(".local-tests"))
        .unwrap();
    let run = temporary.path();
    document::write_new(
        &run.join("manifest.json"),
        &json!({"kind":"cache-query-fixture"}),
    )
    .unwrap();
    state::initialize(run).unwrap();
    let before = common::binding(&run.join("state.json")).unwrap();
    assert_eq!(
        runner::run(
            &backend,
            &CacheOptions {
                run_dir: run.to_owned(),
                operation: CacheOperation::Status,
                request: None
            }
        )
        .unwrap(),
        json!({"status":"cache_unregistered","copy_usable":false,"processes":[]})
    );
    assert_eq!(common::binding(&run.join("state.json")).unwrap(), before);
    let socket = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    socket.set_nonblocking(true).unwrap();
    let request = json!({"previous":{"port":socket.local_addr().unwrap().port()}});
    let mut runtime = Native {
        backend: &backend,
        directory: run,
        owner: None,
    };
    let error = runtime
        .call(
            "redis",
            json!({"request":request,
        "guard":{"request":request,"process":{"password":"private-guard-sentinel"}},
        "parts":["MSETNX","owner","value","sentinel","value"]}),
        )
        .unwrap_err();
    assert!(!error.to_string().contains("private-guard-sentinel"));
    assert!(matches!(socket.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock));
    assert_eq!(common::binding(&run.join("state.json")).unwrap(), before);
    for name in [
        "run.lock",
        "cache-target",
        "intent.json",
        "start.json",
        "runtime.json",
    ] {
        assert!(!run.join(name).exists());
    }
}

fn initialize_capture_source(root: &std::path::Path) {
    use crate::frontend_origin::sources;
    use std::{fs, process::Command};
    fs::create_dir_all(root.join("xtask")).unwrap();
    fs::write(root.join("input.rs"), b"source-input").unwrap();
    fs::write(root.join(sources::POLICY), br#"{"format_version":1,"files":["input.rs","xtask/source-inventory.json"],"trees":[],"excluded_directories":[]}"#).unwrap();
    for arguments in [
        vec!["init", "--quiet"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=source-unit",
            "-c",
            "user.email=source-unit@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "test(source): 创建来源核验夹具",
        ],
    ] {
        let mut command = Command::new("git");
        command.arg("-C").arg(root).args(arguments);
        for (name, _) in std::env::vars_os() {
            if name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
            {
                command.env_remove(name);
            }
        }
        assert!(
            command
                .env("GIT_OPTIONAL_LOCKS", "0")
                .status()
                .unwrap()
                .success()
        );
    }
}

#[test]
fn local_review_preflight_rejects_real_tool_byte_drift_and_preserves_files() {
    use super::review_tools;
    use std::{collections::BTreeMap, fs};

    let parent = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&parent).unwrap();
    let temporary = tempfile::tempdir_in(parent).unwrap();
    let root = temporary.path();
    let mut tools = serde_json::Map::new();
    for role in ["mysql", "mysqldump", "rustfs"] {
        let path = root.join(role);
        fs::write(&path, role).unwrap();
        tools.insert(
            role.into(),
            json!({"path":path,"sha256":digest::file(&path).unwrap().sha256}),
        );
    }
    let review = json!({"ready_for_execution":false,"tools":tools});
    let snapshot = || {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect::<BTreeMap<_, _>>()
    };
    let healthy = snapshot();
    let observed = review_tools::local_tools(&review).unwrap();
    assert_eq!(observed, review["tools"]);
    assert_eq!(snapshot(), healthy);

    // 保持长度、路径和冻结描述不变，只改变实际内容，避免以文件大小代替摘要。
    let dump = root.join("mysqldump");
    fs::write(&dump, b"tampered!").unwrap();
    let changed = snapshot();
    let error = review_tools::local_tools(&review).unwrap_err();
    assert_eq!(error.to_string(), "夹具本机工具字节变化");
    assert_eq!(snapshot(), changed);

    fs::write(&dump, b"mysqldump").unwrap();
    assert_eq!(snapshot(), healthy);
    assert_eq!(review_tools::local_tools(&review).unwrap(), observed);
    assert_eq!(snapshot(), healthy);
}

#[test]
fn native_capture_binds_complete_sources_and_preserves_failure_order_and_io_category() {
    use crate::frontend_origin::manifest;
    use native_tool::NativeTool;
    use std::fs;
    let base = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&base).unwrap();
    let directory = tempfile::tempdir_in(base).unwrap();
    let backend = directory.path();
    let root = backend.join("source");
    initialize_capture_source(&root);
    fs::create_dir(backend.join("config")).unwrap();
    // 仅提供静态格式检查的最小字节，不执行此测试映像。
    let mut image = [0_u8; 20];
    image[..4].copy_from_slice(b"\x7fELF");
    image[4] = 2;
    image[5] = 1;
    image[18] = 62;
    let program = backend.join("observer");
    fs::write(&program, image).unwrap();
    let sha = digest::file(&program).unwrap().sha256;
    let source = crate::recovery::clone::native::source::capture(&root).unwrap();
    let entries = manifest::capture(&root).unwrap();
    let source_sha = manifest::sha256(&entries).unwrap();
    let configuration = format!(
        "[verification.linux_observer]\ndistribution = \"fixture\"\nuser = \"fixture\"\nuid = 1000\nprogram = {}\nlinux_program = \"/fixture/observer\"\nsource_root = {}\nlinux_source_root = \"/fixture/source\"\nsource_commit = \"{}\"\nsha256 = \"{sha}\"\nsource_sha256 = \"{source_sha}\"\n",
        serde_json::to_string(program.to_str().unwrap()).unwrap(),
        serde_json::to_string(root.to_str().unwrap()).unwrap(),
        source.source.snapshot.head,
    );
    let config_path = backend.join("config/tools.toml");
    fs::write(&config_path, &configuration).unwrap();
    let tool = NativeTool::capture(backend).unwrap();
    assert_eq!(tool.sources, entries);
    assert_eq!(tool.source_sha256, source_sha);
    assert_eq!(tool.source_commit, source.source.snapshot.head);
    assert_eq!(tool.sha256, sha);
    tool.finish(backend).unwrap();
    // 映像也无效时，结构/路径门禁仍须先于任何并发检查失败。
    fs::write(&program, []).unwrap();
    for path in [&program, &root] {
        let invalid = configuration.replace(
            &serde_json::to_string(path.to_str().unwrap()).unwrap(),
            "\"relative-path\"",
        );
        fs::write(&config_path, invalid).unwrap();
        assert_eq!(
            NativeTool::capture(backend).unwrap_err().to_string(),
            "观察器 Windows 路径必须是无链接的完整绝对路径"
        );
    }
    #[cfg(unix)]
    {
        let linked = backend.join("linked-source");
        std::os::unix::fs::symlink(&root, &linked).unwrap();
        let invalid = configuration.replace(
            &serde_json::to_string(root.to_str().unwrap()).unwrap(),
            &serde_json::to_string(linked.to_str().unwrap()).unwrap(),
        );
        fs::write(&config_path, invalid).unwrap();
        assert_eq!(
            NativeTool::capture(backend).unwrap_err().to_string(),
            "观察器 Windows 路径必须是无链接的完整绝对路径"
        );
    }
    let wrong_source = configuration
        .replace(&source.source.snapshot.head, &"0".repeat(40))
        .replace(&source_sha, &"0".repeat(64));
    fs::write(&config_path, wrong_source).unwrap();
    fs::write(&program, []).unwrap();
    let error = NativeTool::capture(backend).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    fs::write(&program, image).unwrap();
    assert!(
        NativeTool::capture(backend)
            .unwrap_err()
            .to_string()
            .contains("Linux 观察器来源 HEAD 或 clean 状态不匹配")
    );
    fs::write(
        &config_path,
        configuration.replace(&source_sha, &"0".repeat(64)),
    )
    .unwrap();
    assert_eq!(
        NativeTool::capture(backend).unwrap_err().to_string(),
        "缓存原生工具完整来源摘要不符"
    );
}

#[test]
fn cleanup_diagnostics_preserve_safe_causes_and_reject_untrusted_protocol_fields() {
    use super::cleanup_diagnostic::{self, Operation};
    let error = cleanup_diagnostic::context(
        Operation::Stop,
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private-credential-marker",
        )
        .into(),
    );
    let detail = cleanup_diagnostic::capture(error.as_ref());
    assert_eq!(detail["cause"], "permission_denied");
    assert!(!detail.to_string().contains("private-credential-marker"));
    cleanup_diagnostic::validate(&detail).unwrap();
    let primary = super::diagnostic::failure(Category::ReadinessTimeout, Outcome::Unknown);
    let wrapped = cleanup_diagnostic::decode(primary, &detail).unwrap();
    assert_eq!(
        super::diagnostic::primitive_detail(wrapped.as_ref()).0["category"],
        "readiness_timeout"
    );
    assert!(wrapped.to_string().contains("permission_denied"));
    assert!(!wrapped.to_string().contains("private-credential-marker"));
    for (field, replacement) in [
        ("operation", json!("private-credential-marker")),
        ("cause", json!("private-credential-marker")),
        ("os_code", json!("private-credential-marker")),
        ("diagnostic", json!({"advice":"private-credential-marker"})),
    ] {
        let mut changed = detail.clone();
        changed[field] = replacement;
        assert!(
            !cleanup_diagnostic::validate(&changed)
                .unwrap_err()
                .to_string()
                .contains("private-credential-marker")
        );
    }
    let raw = cleanup_diagnostic::context(
        Operation::CreateDirectory,
        std::io::Error::from_raw_os_error(5).into(),
    );
    let raw = cleanup_diagnostic::capture(raw.as_ref());
    assert_eq!(raw["os_code"], 5);
    cleanup_diagnostic::validate(&raw).unwrap();
}

#[test]
fn readiness_retries_keep_one_budget_and_cannot_promote_a_late_success() {
    let start = Instant::now();
    let deadline = Deadline::new(start, Duration::from_secs(60));
    for seconds in [0, 20, 59] {
        deadline
            .check(
                start + Duration::from_secs(seconds),
                Category::ReadinessTimeout,
            )
            .unwrap();
    }
    for seconds in [60, 61] {
        let error = deadline
            .accepting(
                start + Duration::from_secs(seconds),
                Category::ReadinessTimeout,
            )
            .err()
            .unwrap();
        let detail = error.downcast_ref::<Diagnostic>().unwrap();
        assert_eq!(detail.value()["write_outcome"], "unknown");
        assert_eq!(detail.value()["category"], "readiness_timeout");
    }
}

#[test]
fn acceptance_has_fifteen_seconds_from_ack_and_includes_final_verification() {
    let start = Instant::now();
    let ready = Deadline::new(start, Duration::from_secs(60));
    for ack_seconds in [10, 59] {
        let ack = start + Duration::from_secs(ack_seconds);
        let accepting = ready.accepting(ack, Category::ReadinessTimeout).unwrap();
        // accepted 文件出现不结束预算，后续来源复核仍使用同一截止点。
        accepting
            .check(ack + Duration::from_secs(2), Category::AcceptanceTimeout)
            .unwrap();
        accepting
            .check(ack + Duration::from_secs(14), Category::AcceptanceTimeout)
            .unwrap();
        for seconds in [15, 16] {
            let error = accepting
                .check(
                    ack + Duration::from_secs(seconds),
                    Category::AcceptanceTimeout,
                )
                .unwrap_err();
            assert_eq!(
                error.downcast_ref::<Diagnostic>().unwrap().value()["category"],
                "acceptance_timeout"
            );
        }
    }
}

pub(super) fn native_request(mut request: Value) -> Value {
    let sources = json!([{"path":"xtask/source-inventory.json","sha256":"a".repeat(64)}]);
    request.as_object_mut().unwrap().remove("python");
    request["format_version"] = json!(2);
    request["native_tool"] = json!({"distribution":request["distribution"],"user":"fixture","uid":1000,
        "program":"D:/fixture/xtask","linux_program":"/mnt/d/fixture/xtask","source_root":"D:/fixture/source",
        "linux_source_root":"/mnt/d/fixture/source","source_commit":"b".repeat(40),"sha256":"c".repeat(64),
        "source_sha256":digest::plan_hash(&sources).unwrap(),"sources":sources});
    request
}

#[test]
fn v2_independent_shape_rejects_private_overrides_paths_incomplete_owner_and_bad_sources() {
    let request = native_request(super::tests::request());
    model::shape(&request).unwrap();
    for (path, value) in [
        ("/format_version", json!(true)),
        ("/timeout_seconds", json!(181)),
        ("/port", json!(true)),
        ("/directory/device", json!(true)),
        ("/directory/inode", json!(-1)),
        ("/configuration/bytes", json!(-1)),
        ("/configuration/path", json!("D:/another/redis.conf")),
        ("/native_tool/uid", json!(false)),
        ("/native_tool/linux_program", json!("/mnt/d/../other")),
        ("/native_tool/program", json!("relative")),
        ("/native_tool/source_sha256", json!("d".repeat(64))),
        ("/previous_boot_id", Value::Null),
    ] {
        let mut wrong = request.clone();
        *wrong.pointer_mut(path).unwrap() = value;
        assert!(model::shape(&wrong).is_err(), "{path}");
    }
    let mut unknown = request;
    unknown["password_env"] = json!("private-marker");
    assert!(model::shape(&unknown).is_err());
    let mut old_execution = super::tests::request();
    old_execution["format_version"] = json!(2);
    assert!(model::shape(&old_execution).is_err());
}

#[test]
fn native_successor_preserves_uid_paths_resources_and_original_full_source_digest() {
    let original = native_request(super::tests::request());
    let mut tools = json!({"wsl":original["wsl"],"native_tool":original["native_tool"],
        "redis_server":{"distribution":original["distribution"],"resolved_path":original["executable"],"sha256":original["sha256"]}});
    tools["native_tool"]["sha256"] = json!("e".repeat(64));
    tools["native_tool"]["source_commit"] = json!("f".repeat(40));
    let next = native_tool::successor(&original, &tools).unwrap();
    assert_eq!(
        next["native_tool"]["sha256"],
        tools["native_tool"]["sha256"]
    );
    assert_eq!(next["directory"], original["directory"]);
    for key in [
        "uid",
        "distribution",
        "user",
        "program",
        "linux_program",
        "source_root",
        "linux_source_root",
    ] {
        let mut wrong = tools.clone();
        wrong["native_tool"][key] = if key == "uid" {
            json!(1001)
        } else {
            json!("changed")
        };
        assert!(native_tool::successor(&original, &wrong).is_err(), "{key}");
    }
    let mut wrong = tools;
    wrong["native_tool"]["sources"][0]["sha256"] = json!("0".repeat(64));
    assert!(native_tool::successor(&original, &wrong).is_err());
}

#[test]
fn stop_failure_observes_once_and_preserves_original_failure_if_alive_or_unknown() {
    for observation in [Some(false), Some(true), None] {
        let mut actions = Vec::new();
        let result = host_control::stop_once(|action| {
            actions.push(action.to_string());
            match action {
                "stop" => Err(process::ControlFailure(Some(17)).into()),
                "check" => observation
                    .map(|alive| json!({"alive":alive}))
                    .ok_or_else(|| "unknown".into()),
                _ => panic!("不得重放或执行其他动作"),
            }
        });
        assert_eq!(actions, ["stop", "check"]);
        if observation == Some(false) {
            assert_eq!(result.unwrap(), (json!({"alive":false}), true));
        } else {
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast_ref::<process::ControlFailure>()
                    .unwrap()
                    .0,
                Some(17)
            );
        }
    }
    let mut calls = 0;
    host_control::stop_once(|_| {
        calls += 1;
        Ok(json!({"alive":false}))
    })
    .unwrap();
    assert_eq!(calls, 1);
}

#[test]
fn diagnostics_only_accept_fixed_fields_and_never_disclose_remote_advice_or_categories() {
    for (category, phase, field) in [
        (
            Category::ParentAuthorizationRejected,
            "exec-authorization",
            "linux-intent.supervisor",
        ),
        (
            Category::ProcessReceiptTimeout,
            "process-receipt",
            "linux-process.json",
        ),
        (Category::AcceptanceTimeout, "acceptance", "accepted.json"),
        (Category::ReadinessTimeout, "readiness", "timeout_seconds"),
        (
            Category::AcknowledgementTimeout,
            "acknowledgement",
            "acknowledged",
        ),
    ] {
        let diagnostic = Diagnostic {
            category,
            write_outcome: Outcome::Unknown,
            os_code: None,
        };
        let expected = diagnostic.value();
        let decoded = Diagnostic::decode(&expected).unwrap();
        assert_eq!(expected["phase"], phase);
        assert_eq!(expected["field"], field);
        assert_eq!(expected["write_outcome"], "unknown");
        assert_eq!(decoded.value(), expected);
        assert!(decoded.to_string().contains(field));
        for field in ["advice", "phase", "field", "category", "write_outcome"] {
            let mut wrong = expected.clone();
            wrong[field] = json!("private-password-marker");
            let error = Diagnostic::decode(&wrong).unwrap_err().to_string();
            assert!(!error.contains("private-password-marker"));
        }
        let response = json!({"ok":false,"error":"NativeCacheRejected","diagnostic":expected});
        Diagnostic::decode_output(&serde_json::to_vec(&response).unwrap()).unwrap();
    }
    assert!(Diagnostic::decode_output(&vec![b'x'; 16385]).is_err());
}
