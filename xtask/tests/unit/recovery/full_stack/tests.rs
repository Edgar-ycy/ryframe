use super::*;
use serde_json::json;
use std::fs;

#[derive(Default)]
struct Fake {
    events: Vec<String>,
    fail: Option<String>,
}
impl Port for Fake {
    fn leaf(&mut self, op: &str) -> Result<Value> {
        self.events.push(op.into());
        if self
            .fail
            .as_deref()
            .is_some_and(|v| v.split('|').any(|v| v == op))
        {
            return Err("指定阶段失败".into());
        }
        if op == "register" {
            return Ok(json!({"worker_ready_url":"http://127.0.0.1:19210/readyz"}));
        }
        if op == "build" {
            let root = std::env::current_exe()?.parent().unwrap().to_path_buf();
            return Ok(
                json!({"ryframe-reset":root.join("reset"),"ryframe-migrate":root.join("migrate")}),
            );
        }
        if op == "api-stop" {
            return Ok(json!({"state":"stopped"}));
        }
        Ok(json!({}))
    }
    fn command(&mut self, _program: &str, args: &[String], _log: Option<&Path>) -> Result<String> {
        let text = args.join(" ");
        self.events.push(text.clone());
        if self.fail.as_ref().is_some_and(|v| text.contains(v)) {
            return Err("指定命令失败".into());
        }
        Ok(format!("plan_hash={}\n", "a".repeat(64)))
    }
    fn sleep(&mut self, _duration: Duration) -> Result<()> {
        Ok(())
    }
    fn storage(&mut self, _frozen: &crate::ci::databases::snapshot::Frozen) -> Result<()> {
        self.events.push("storage".into());
        Ok(())
    }
    fn databases(&mut self, _frozen: &crate::ci::databases::snapshot::Frozen) -> Result<()> {
        self.events.push("databases".into());
        Ok(())
    }
}

fn fixture() -> (tempfile::TempDir, context::Context) {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let temp = tempfile::tempdir_in(local).unwrap();
    let root = temp.path().to_path_buf();
    let output = root.join("runtime");
    fs::create_dir(&output).unwrap();
    fs::create_dir(output.join("config")).unwrap();
    fs::write(
        output.join("config/app.test.toml"),
        "[app]\nhost='127.0.0.1'\nport=18210\n",
    )
    .unwrap();
    let control = json!({"mysql_container_id":"a".repeat(64),"redis_container_id":"b".repeat(64),"s3_container_id":"c".repeat(64)});
    (
        temp,
        context::Context {
            root,
            output,
            control,
            run: "42".into(),
            attempt: "3".into(),
        },
    )
}

#[test]
fn startup_requires_confirmed_api_launch_and_observation_before_worker_and_never_replays() {
    let (_temp, context) = fixture();
    let mut port = Fake::default();
    lifecycle::start(&context, &mut port).unwrap();
    assert_eq!(
        port.events,
        ["register", "api-launch", "api-probe", "worker-start"]
    );
    assert!(lifecycle::start(&context, &mut port).is_err());
    assert_eq!(port.events.iter().filter(|v| *v == "api-launch").count(), 1);
}

#[test]
fn startup_failure_preserves_receipt_and_never_replays_or_stops_worker() {
    for failure in ["api-launch", "api-probe", "worker-start"] {
        let (_temp, context) = fixture();
        let mut port = Fake {
            fail: Some(failure.into()),
            ..Default::default()
        };
        let error = lifecycle::start(&context, &mut port)
            .unwrap_err()
            .to_string();
        let owns_api = failure != "api-launch";
        if owns_api {
            assert_eq!(port.events.last().map(String::as_str), Some("api-stop"));
        } else {
            assert_eq!(port.events, ["register", "api-launch"]);
            assert!(error.contains("显式执行 collect"));
        }
        assert!(!port.events.iter().any(|v| v == "worker-stop"));
        assert_eq!(
            port.events.iter().any(|v| v == "worker-start"),
            failure == "worker-start"
        );
        let path = context.output.join("ci-start.json");
        let bytes = fs::read(&path).unwrap();
        let receipt: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(receipt["state"], "start-failed");
        assert_eq!(receipt["api_cleanup_confirmed"], owns_api);
        assert_eq!(receipt["automatic_retry_permitted"], false);
        assert!(lifecycle::start(&context, &mut port).is_err());
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert_eq!(port.events.iter().filter(|v| *v == "api-launch").count(), 1);
    }
}

#[test]
fn collect_reports_missing_authority_and_still_collects_diagnostics() {
    let (_temp, context) = fixture();
    fs::write(context.output.join("worker.json"), "{}").unwrap();
    let mut port = Fake::default();
    assert!(lifecycle::collect(&context, &mut port).is_err());
    assert!(!port.events.iter().any(|v| v == "worker-stop"));
    assert!(port.events.iter().any(|v| v == "--stop-server"));
    assert!(context.output.join("cleanup-errors.log").is_file());
}

#[test]
fn collect_failure_does_not_prevent_other_owned_cleanup() {
    let (_temp, context) = fixture();
    fs::write(context.output.join("runtime.json"), "{}").unwrap();
    fs::write(context.output.join("api-tree.json"), "{}").unwrap();
    let mut port = Fake {
        fail: Some("worker-stop|api-stop|api-port-free".into()),
        ..Default::default()
    };
    assert!(lifecycle::collect(&context, &mut port).is_err());
    assert!(port.events.starts_with(&[
        "worker-stop".into(),
        "api-stop".into(),
        "api-port-free".into()
    ]));
    assert!(port.events.iter().any(|v| v == "--stop-server"));
    let errors = fs::read_to_string(context.output.join("cleanup-errors.log")).unwrap();
    assert!(errors.contains("API 回收未确认"));
    assert!(errors.contains("API 端口未确认释放"));
}

#[test]
fn invalid_storage_and_redis_configuration_fail_before_work() {
    let value = json!({"jobs":{"mode":"external"},"redis":{"database":14},"reset":{"redis_outside_sentinel_key":"outside"},
        "object_storage":{"backend":"s3","endpoint":"http://127.0.0.1:19000"}});
    prepare::validate(&value).unwrap();
    for endpoint in [
        "http://remote.example",
        "http://secret@localhost",
        "http://localhost/prefix",
        "http://localhost/?secret=x",
    ] {
        let mut changed = value.clone();
        changed["object_storage"]["endpoint"] = json!(endpoint);
        assert!(prepare::validate(&changed).is_err());
    }
    for number in [json!(true), json!(-1), json!(16), json!("14")] {
        let mut changed = value.clone();
        changed["redis"]["database"] = number;
        assert!(prepare::validate(&changed).is_err());
    }
}

#[test]
fn reset_requires_exact_hash_and_uses_last_plan() {
    assert!(prepare::plan_hash("plan_hash=bad\n").is_err());
    assert_eq!(
        prepare::plan_hash(&format!(
            "plan_hash={}\nplan_hash={}\n",
            "a".repeat(64),
            "b".repeat(64)
        ))
        .unwrap(),
        "b".repeat(64)
    );
}

#[test]
fn frozen_database_input_is_captured_once_and_stages_keep_original_order() {
    let (_temp, context) = fixture();
    // 规划必须发生在输出配置发布之前。
    fs::remove_dir_all(context.output.join("config")).unwrap();
    let directory = context.root.join(".local-tests/full-stack-config");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("app.toml"), "[tenant_data]\n").unwrap();
    let text = "scope_id='ci-42-3'\nsnowflake_worker_id=3\n[database.primary]\nhost='127.0.0.1'\nport=3306\ndatabase='ryframe_e2e_42_3'\nusername='root'\npassword='private-password'\ntls_mode='required'\n[redis]\ndatabase=14\n[reset]\nredis_outside_sentinel_key='outside'\n";
    fs::write(directory.join("app.test.toml"), text).unwrap();
    let frozen = crate::ci::databases::snapshot::Frozen::capture(
        &context.root,
        &context.output,
        "42",
        "3",
        &"a".repeat(64),
    )
    .unwrap();
    fs::write(directory.join("app.test.toml"), "scope_id='foreign'").unwrap();
    assert_eq!(frozen.settings()["scope_id"], "ci-42-3");
    let mut port = Fake::default();
    prepare::phases(&context, &mut port, &frozen).unwrap();
    assert_eq!(&port.events[..3], ["build", "storage", "databases"]);
    assert!(port.events[3].contains("redis-cli"));
    assert!(
        port.events
            .iter()
            .any(|v| v.contains("RESET-RYFRAME-test-ci-42-3"))
    );
    assert!(
        port.events
            .last()
            .unwrap()
            .ends_with("tenant-data verify --all")
    );
}

#[test]
fn worker_failure_with_unconfirmed_api_cleanup_never_claims_safe_completion() {
    let (_temp, context) = fixture();
    let mut port = Fake {
        fail: Some("worker-start|api-stop".into()),
        ..Default::default()
    };
    let error = lifecycle::start(&context, &mut port)
        .unwrap_err()
        .to_string();
    assert!(error.contains("回收未确认"));
    let receipt: Value =
        serde_json::from_slice(&fs::read(context.output.join("ci-start.json")).unwrap()).unwrap();
    assert_eq!(receipt["api_cleanup_confirmed"], false);
}

#[test]
fn failed_reset_keeps_exact_stage_and_forbids_whole_prepare_replay() {
    let (_temp, context) = fixture();
    fs::remove_dir_all(context.output.join("config")).unwrap();
    let directory = context.root.join(".local-tests/full-stack-config");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("app.toml"), "[tenant_data]\n").unwrap();
    let text = format!(
        "scope_id='ci-42-3'\nsnowflake_worker_id=3\n[database.primary]\nhost='127.0.0.1'\nport=3306\ndatabase='ryframe_e2e_42_3'\nusername='root'\npassword='private-password'\ntls_mode='required'\n[jobs]\nmode='external'\n[redis]\ndatabase=14\n[reset]\nredis_outside_sentinel_key='outside'\n[object_storage]\nbackend='local'\nlocal_base_dir={}\n",
        json!(context.root.join("storage"))
    );
    fs::write(directory.join("app.test.toml"), text).unwrap();
    let frozen = crate::ci::databases::snapshot::Frozen::capture(
        &context.root,
        &context.output,
        "42",
        "3",
        &"a".repeat(64),
    )
    .unwrap();
    let mut port = Fake {
        fail: Some("execute".into()),
        ..Default::default()
    };
    assert!(prepare::run_frozen(&context, &mut port, &frozen).is_err());
    let receipt =
        crate::recovery::clone::native::json::read(&context.output.join("ci-prepare.json"))
            .unwrap();
    assert_eq!(receipt["state"], "needs-reconciliation");
    assert_eq!(receipt["phase"], "reset");
    assert!(!receipt.to_string().contains("private-password"));
    let count = port.events.len();
    assert!(prepare::run_frozen(&context, &mut port, &frozen).is_err());
    assert_eq!(port.events.len(), count);
}

#[test]
fn login_budget_reuses_native_authority_and_never_overwrites_existing_tool_file() {
    let (_temp, context) = fixture();
    let output = context.output.join("budget.json");
    fs::write(
        context.output.join("config/app.test.toml"),
        "[rate_limit]\nwindow_secs=60\n[rate_limit.api_limits]\n'POST /api/v1/auth/login'=7\n",
    )
    .unwrap();
    context.rate_limit(&output).unwrap();
    let value = crate::recovery::clone::native::json::read(&output).unwrap();
    assert_eq!(value["loginCapacity"], 7);
    assert!(context.rate_limit(&output).is_err());
    fs::write(
        context.output.join("config/app.test.toml"),
        "[rate_limit.api_limits]\n'POST /api/v1/auth/login'=0\n",
    )
    .unwrap();
    assert!(
        context
            .rate_limit(&context.output.join("invalid.json"))
            .is_err()
    );
    assert!(!context.output.join("invalid.json").exists());
}

#[test]
fn invalid_api_binding_fails_before_registration_or_process_creation() {
    for text in [
        "[app]\nhost='192.0.2.1'\nport=18210\n",
        "[app]\nhost='127.0.0.1'\nport=0\n",
        "[app]\nhost='127.0.0.1'\nport=65536\n",
    ] {
        let (_temp, context) = fixture();
        fs::write(context.output.join("config/app.test.toml"), text).unwrap();
        let mut port = Fake::default();
        assert!(lifecycle::start(&context, &mut port).is_err());
        assert!(port.events.is_empty());
        assert!(!context.output.join("ci-start.json").exists());
    }
}

#[test]
fn shared_api_worker_port_never_launches_either_product() {
    let (_temp, context) = fixture();
    fs::write(
        context.output.join("config/app.test.toml"),
        "[app]\nhost='127.0.0.1'\nport=19210\n",
    )
    .unwrap();
    let mut port = Fake::default();
    assert!(lifecycle::start(&context, &mut port).is_err());
    assert_eq!(port.events, ["register"]);
    assert!(!context.output.join("ci-start.json").exists());
}
