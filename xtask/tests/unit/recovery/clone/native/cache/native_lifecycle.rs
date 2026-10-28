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
    let mut unknown = request.clone();
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
