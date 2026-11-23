use super::*;
use crate::recovery::{
    clone::native::digest,
    fixture_control::environment::{guards, protocol::tests as fixtures},
};
use std::fs;

struct Fixture {
    directory: tempfile::TempDir,
    root: std::path::PathBuf,
    input: std::path::PathBuf,
    output: std::path::PathBuf,
    review: Value,
    observed: Value,
}

impl Fixture {
    fn new() -> Self {
        let root = crate::workspace::root_dir();
        let local = root.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("environment-review-")
            .tempdir_in(local)
            .unwrap();
        let mut review = fixtures::review(directory.path());
        review["ready_for_execution"] = json!(false);
        let sources = json!([{"path":"xtask/source-inventory.json","sha256":"a".repeat(64)}]);
        let mut observed = fixtures::ready(review.clone())["preflight"]["tools"].clone();
        observed.as_object_mut().unwrap().remove("redis_python");
        observed["native_tool"] = json!({
            "distribution":"Ubuntu","user":"fixture","uid":1000,
            "program":"D:/fixture/xtask","linux_program":"/mnt/d/fixture/xtask",
            "source_root":"D:/fixture/source","linux_source_root":"/mnt/d/fixture/source",
            "source_commit":"b".repeat(40),"sha256":"c".repeat(64),
            "source_sha256":digest::plan_hash(&sources).unwrap(),"sources":sources,
        });
        let input = directory.path().join("predecessor.json");
        let output = directory.path().join("ready.json");
        write_new(&input, &review).unwrap();
        Self {
            directory,
            root,
            input,
            output,
            review,
            observed,
        }
    }

    fn renew(&self) -> Result<Value> {
        renew_with_observer(&self.root, &self.input, &self.output, |_, _| {
            Ok(self.observed.clone())
        })
    }

    fn windows_tools(&self) -> Value {
        let mut tools = self.observed.clone();
        let entries = tools.as_object_mut().unwrap();
        entries.remove("wsl");
        entries.remove("native_tool");
        tools["redis_server"] = json!({
            "platform":"windows",
            "path":"D:/tools/redis-server.exe",
            "sha256":"b".repeat(64),
            "version":"Redis server v=7.4.11 sha=00000000:0 malloc=libc bits=64 build=5bd2587910691cb8",
            "dlls":{"msys-2.0.dll":"a".repeat(64)},
        });
        tools
    }
}

fn measurement_review(review: &Value, tools: &Value) -> Value {
    let future = Path::new(review["future_root"].as_str().unwrap());
    let mut candidate = review["scopes"]["candidate"].clone();
    candidate["scope_id"] = json!("fixture-b1-measurement");
    candidate["runtime_dir"] = json!(future.join("runtime-candidate"));
    candidate["identity_ledger"] = json!(future.join("identities-candidate"));
    candidate["objects"]["endpoint"] = review["services"]["rustfs"]["api"].clone();
    candidate["redis"]["namespace"] = json!("ryframe:{fixture-b1-measurement}:");
    candidate["redis"]["ownership_key"] = json!("ryframe:{fixture-b1-measurement}:.ryframe-owner");
    candidate["redis"]["ownership_value"] = json!("ryframe-owner:v1:fixture-b1-measurement:redis");
    let mut review = review.clone();
    review.as_object_mut().unwrap().remove("preflight");
    review["kind"] = json!("review-only-b1-runtime-measurement-with-readonly-preflight");
    review["ready_for_execution"] = json!(false);
    review["scopes"] = json!({"candidate":candidate});
    review["services"]["redis"]["endpoint"] = json!("127.0.0.1:16390");
    review["services"]["redis"]["directory"] = json!(future.join("redis"));
    let rustfs = review["services"]["rustfs"].as_object_mut().unwrap();
    rustfs.insert("data_dir".into(), json!(future.join("rustfs")));
    rustfs.remove("scope_id");
    rustfs.remove("process_receipt");
    review["tools"] = tools.clone();
    for role in ["mysql", "mysqldump", "rustfs"] {
        review["tools"][role]["sha256"] = json!("a".repeat(64));
    }
    review
}

#[test]
fn measurement_preflight_and_environment_plan_use_only_candidate() {
    use crate::{
        cli::{FixtureEnvironmentCommand, FixtureEnvironmentInputs, FixtureSide},
        recovery::fixture_control::{review, services::lifecycle::query, source_pair::execution},
    };
    let sources = execution::tests::Fixture::new();
    let (directory, legacy) = fixtures::published_plan(&sources);
    let execution_root = Path::new(legacy["execution_backend"].as_str().unwrap());
    let reference = execution_root.join(".local-tests/reference-fixture");
    fs::create_dir_all(&reference).unwrap();
    let predecessor =
        crate::recovery::clone::native::json::read(&directory.join("review.json")).unwrap();
    let pending = measurement_review(&predecessor, &Fixture::new().windows_tools());
    let review_input = directory.join("measurement-pending.json");
    let review_ready = directory.join("measurement-ready.json");
    write_new(&review_input, &pending).unwrap();
    let ready = renew_with_observer(&sources.backend, &review_input, &review_ready, |_, _| {
        Ok(pending["tools"].clone())
    })
    .unwrap();
    let future = Path::new(ready["future_root"].as_str().unwrap());
    let run = future.join("service-run");
    assert_eq!(
        ready["services"]["rustfs"]["scope_id"],
        "services-fixture-b1-measurement"
    );
    assert_eq!(
        query::dispatch(&sources.backend, "service-run", &json!({"review":ready})).unwrap(),
        json!(run)
    );
    assert!(!future.exists());
    assert!(
        review::reconciled::verify(&sources.backend, &ready)
            .unwrap()
            .is_none()
    );
    let execution_source = sources.output.join("execution-source.json");
    let maintenance_build = directory.join("maintenance_build.json");
    let mut args = json!({"backend":sources.backend,"review":review_ready,
        "execution_source":execution_source,"maintenance_build":maintenance_build,"side":"candidate"});
    let plan = super::super::plan::create(&args).unwrap();
    assert_eq!(plan["scope_id"], "fixture-b1-measurement");
    assert_eq!(plan["databases"].as_array().unwrap().len(), 4);
    super::super::plan::inputs(&json!({"backend":sources.backend,"plan":plan})).unwrap();
    args["side"] = json!("seed");
    assert!(super::super::plan::create(&args).is_err());
    let output = reference.join("measurement-prepare");
    let command = FixtureEnvironmentCommand::Prepare {
        inputs: FixtureEnvironmentInputs {
            review: review_ready,
            execution_source,
            maintenance_build,
            side: FixtureSide::Candidate,
        },
        output: output.clone(),
        secrets_dir: None,
    };
    let error = super::super::prepare::run(&sources.backend, &command).unwrap_err();
    assert!(error.to_string().contains("--secrets-dir"));
    assert!(!output.exists());
}

#[test]
fn measurement_review_rejects_extra_roles_linux_and_legacy_reconciliation() {
    use crate::recovery::{
        clone::native::target_verify::input::request_review, fixture_control::review,
    };
    let fixture = Fixture::new();
    let mut ready = measurement_review(&fixture.review, &fixture.windows_tools());
    ready["ready_for_execution"] = json!(true);
    request_review::validate(&ready).unwrap();
    let mut extra = ready.clone();
    extra["scopes"]["seed"] = fixture.review["scopes"]["seed"].clone();
    assert!(request_review::validate(&extra).is_err());
    let mut wrong = ready.clone();
    wrong["scopes"] = json!({"base":ready["scopes"]["candidate"]});
    assert!(request_review::validate(&wrong).is_err());
    let mut linux = ready.clone();
    linux["tools"]["redis_server"] = fixture.review["tools"]["redis_server"].clone();
    assert!(
        request_review::validate(&linux)
            .unwrap_err()
            .to_string()
            .contains("Windows Redis")
    );
    let mut overlap = ready.clone();
    overlap["reference"]["protected_target"]["databases"] = json!([{
        "server_uuid":ready["scopes"]["candidate"]["databases"][0]["expected_server_uuid"],
        "database":ready["scopes"]["candidate"]["databases"][0]["database"],
    }]);
    assert!(request_review::validate(&overlap).is_err());
    let mut legacy = ready.clone();
    legacy["kind"] = fixture.review["kind"].clone();
    assert!(review::validate_review(&legacy).is_err());
    ready["sources"] = json!({"reconciled_seed":{}});
    assert!(review::reconciled::verify(&fixture.root, &ready).is_err());
}

#[test]
fn pending_and_ready_refresh_preserve_history_and_raw_predecessor_binding() {
    let fixture = Fixture::new();
    let original = fs::read(&fixture.input).unwrap();
    let binding = file_binding(&fixture.input, "原始输入").unwrap();
    let ready = fixture.renew().unwrap();
    assert_eq!(ready["preflight"]["supersedes"], binding);
    assert_eq!(ready["preflight"]["tools"], fixture.observed);
    assert_eq!(
        ready["tools"]["historical"],
        fixture.review["tools"]["historical"]
    );
    assert!(ready["tools"].get("redis_python").is_none());
    assert_eq!(ready["ready_for_execution"], true);
    guards::preflight_binding(&ready).unwrap();
    assert_eq!(fs::read(&fixture.input).unwrap(), original);
    assert_eq!(
        read_json(&fixture.output, "已发布收据", LIMIT).unwrap(),
        ready
    );

    #[cfg(windows)]
    let ready = {
        let mut ready = ready;
        let future = ready["future_root"].as_str().unwrap().replace('\\', "/");
        let frozen = ready["services"]["rustfs"]["process_receipt"]
            .as_str()
            .unwrap()
            .replace('/', "\\");
        ready["future_root"] = json!(future);
        ready["services"]["rustfs"]["process_receipt"] = json!(frozen);
        guards::preflight_binding(&ready).unwrap();
        assert_ne!(
            ready["services"]["rustfs"]["process_receipt"],
            json!(
                guards::service_run(&ready)
                    .unwrap()
                    .join("rustfs")
                    .join("process.json")
            )
        );
        fs::write(&fixture.output, serde_json::to_vec(&ready).unwrap()).unwrap();
        ready
    };
    let frozen_services = ready["services"].clone();
    let next = fixture.directory.path().join("refreshed.json");
    let mut observed = fixture.observed.clone();
    observed["native_tool"]["sha256"] = json!("d".repeat(64));
    let renewed = renew_with_observer(&fixture.root, &fixture.output, &next, |_, review| {
        assert_eq!(review, &ready);
        Ok(observed.clone())
    })
    .unwrap();
    assert_eq!(
        renewed["preflight"]["supersedes"],
        file_binding(&fixture.output, "ready").unwrap()
    );
    assert_eq!(renewed["tools"]["native_tool"], observed["native_tool"]);
    assert_eq!(renewed["services"], frozen_services);
    guards::successor(&ready, &renewed).unwrap();
}

#[test]
fn windows_review_preserves_pending_input_and_requires_same_platform_for_ready_successors() {
    let fixture = Fixture::new();
    let original = fs::read(&fixture.input).unwrap();
    let binding = file_binding(&fixture.input, "原始输入").unwrap();
    let linux = receipt(&fixture.review, &fixture.observed, &binding).unwrap();
    let windows = fixture.windows_tools();
    let ready = renew_with_observer(
        &fixture.root,
        &fixture.input,
        &fixture.output,
        |_, input| {
            assert_eq!(input, &fixture.review);
            Ok(windows.clone())
        },
    )
    .unwrap();
    assert_eq!(ready["preflight"]["supersedes"], binding);
    assert_eq!(ready["preflight"]["tools"], windows);
    assert_eq!(guards::preflight_binding(&ready).unwrap(), windows);
    assert_eq!(
        ready["tools"]["historical"],
        fixture.review["tools"]["historical"]
    );
    for key in ["wsl", "native_tool", "redis_python"] {
        assert!(ready["tools"].get(key).is_none(), "{key}");
    }
    assert_eq!(fs::read(&fixture.input).unwrap(), original);

    let next = fixture.directory.path().join("refreshed-windows.json");
    let refreshed = renew_with_observer(&fixture.root, &fixture.output, &next, |_, input| {
        assert_eq!(input, &ready);
        Ok(windows.clone())
    })
    .unwrap();
    assert_eq!(refreshed["services"], ready["services"]);
    assert_eq!(
        refreshed["preflight"]["supersedes"],
        file_binding(&fixture.output, "Windows ready").unwrap()
    );
    for (previous, successor) in [(&linux, &ready), (&ready, &linux)] {
        let error = guards::successor(previous, successor).unwrap_err();
        assert!(error.to_string().contains("请新建隔离场景"));
    }
}

#[test]
fn windows_preflight_rejects_mixed_tools_and_changed_dll_observation() {
    let fixture = Fixture::new();
    let original = fs::read(&fixture.input).unwrap();
    let binding = file_binding(&fixture.input, "原始输入").unwrap();
    let windows = fixture.windows_tools();
    let ready = receipt(&fixture.review, &windows, &binding).unwrap();
    for key in ["wsl", "native_tool", "redis_python"] {
        let mut mixed = ready.clone();
        mixed["tools"][key] = json!({"sha256":"c".repeat(64)});
        assert!(guards::preflight_binding(&mixed).is_err(), "{key}");
        let mut mixed = ready.clone();
        mixed["preflight"]["tools"][key] = json!({"sha256":"c".repeat(64)});
        assert!(guards::preflight_binding(&mixed).is_err(), "{key}");
    }
    let mut mixed = ready;
    mixed["tools"]["redis_server"]["distribution"] = json!("Ubuntu");
    mixed["preflight"]["tools"]["redis_server"] = mixed["tools"]["redis_server"].clone();
    assert!(guards::preflight_binding(&mixed).is_err());

    let mut observations = 0;
    let error = renew_with_observer(&fixture.root, &fixture.input, &fixture.output, |_, _| {
        observations += 1;
        let mut tools = windows.clone();
        if observations == 2 {
            tools["redis_server"]["dlls"]["msys-2.0.dll"] = json!("d".repeat(64));
        }
        Ok(tools)
    })
    .unwrap_err();
    assert!(error.to_string().contains("ready 签发前发生变化"));
    assert!(!fixture.output.exists());
    assert_eq!(fs::read(&fixture.input).unwrap(), original);
}

#[test]
fn observation_and_predecessor_drift_never_publish_a_receipt() {
    // 两个观察窗口分别覆盖内容漂移、语义相同的原始字节漂移和观察失败。
    for stage in [1, 2] {
        for change in ["content", "raw", "tools", "failure"] {
            let fixture = Fixture::new();
            let mut calls = 0;
            let result =
                renew_with_observer(&fixture.root, &fixture.input, &fixture.output, |_, _| {
                    calls += 1;
                    let mut observed = fixture.observed.clone();
                    if calls == stage {
                        match change {
                            "content" => fs::write(&fixture.input, b"{}").unwrap(),
                            "raw" => {
                                let mut bytes = fs::read(&fixture.input).unwrap();
                                bytes.extend_from_slice(b" \n");
                                fs::write(&fixture.input, bytes).unwrap();
                            }
                            "tools" => observed["rustfs"]["sha256"] = json!("changed"),
                            "failure" => {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::PermissionDenied,
                                    "do-not-echo-credential",
                                )
                                .into());
                            }
                            _ => unreachable!(),
                        }
                    }
                    Ok(observed)
                });
            assert!(result.is_err(), "{stage}/{change}");
            let error = result.unwrap_err();
            let message = error.to_string();
            assert!(message.contains("环境审阅阶段"));
            assert!(message.contains("--review"));
            assert!(message.contains("verification.windows_redis"));
            assert!(!message.contains("do-not-echo-credential"));
            assert!(error.source().is_some());
            if change == "failure" {
                let expected = if stage == 1 {
                    "首次工具观察"
                } else {
                    "终末工具观察"
                };
                assert!(message.contains(expected));
                assert!(message.contains("PermissionDenied"));
            }
            assert!(!fixture.output.exists(), "{stage}/{change}");
        }
    }
    for malformed in [
        "services-array",
        "rustfs-string",
        "ready-missing",
        "ready-string",
        "ready-scope",
        "ready-process",
    ] {
        let fixture = Fixture::new();
        let mut invalid = fixture.review.clone();
        match malformed {
            "services-array" => invalid["services"] = json!([]),
            "rustfs-string" => invalid["services"]["rustfs"] = json!("invalid"),
            "ready-missing" => {
                invalid
                    .as_object_mut()
                    .unwrap()
                    .remove("ready_for_execution");
            }
            "ready-string" => invalid["ready_for_execution"] = json!("false"),
            "ready-scope" => {
                invalid = fixtures::ready(invalid);
                invalid["services"]["rustfs"]["scope_id"] = json!("services-foreign");
            }
            "ready-process" => {
                invalid = fixtures::ready(invalid);
                invalid["services"]["rustfs"]["process_receipt"] = json!("D:/foreign/process.json");
            }
            _ => unreachable!(),
        }
        fs::write(&fixture.input, serde_json::to_vec(&invalid).unwrap()).unwrap();
        let mut observations = 0;
        let error = renew_with_observer(&fixture.root, &fixture.input, &fixture.output, |_, _| {
            observations += 1;
            Ok(fixture.observed.clone())
        })
        .unwrap_err();
        assert_eq!(observations, 0);
        assert!(error.to_string().contains("输入核验"));
        assert!(!fixture.output.exists());
    }
}

#[test]
fn existing_output_missing_parent_and_publication_race_do_not_overwrite() {
    let fixture = Fixture::new();
    fs::write(&fixture.output, b"existing").unwrap();
    assert!(fixture.renew().is_err());
    assert_eq!(fs::read(&fixture.output).unwrap(), b"existing");
    let missing = fixture.directory.path().join("missing/ready.json");
    assert!(
        renew_with_observer(&fixture.root, &fixture.input, &missing, |_, _| {
            Ok(fixture.observed.clone())
        })
        .is_err()
    );
    assert!(!missing.parent().unwrap().exists());

    let raced = fixture.directory.path().join("raced.json");
    let mut calls = 0;
    let error = renew_with_observer(&fixture.root, &fixture.input, &raced, |_, _| {
        calls += 1;
        if calls == 2 {
            fs::write(&raced, b"other-owner").unwrap();
        }
        Ok(fixture.observed.clone())
    })
    .unwrap_err();
    let message = error.to_string();
    for expected in [
        "本地收据发布",
        "可能已存在或包含部分文件",
        "保留输出与原审阅",
        "禁止覆盖或自动重放",
        "不得盲目重试",
    ] {
        assert!(message.contains(expected), "{message}");
    }
    assert_eq!(fs::read(&raced).unwrap(), b"other-owner");
}
