//! 原 Python 请求比较场景迁入纯 Rust；路径/配置采集由适配测试覆盖。
use super::super::{markers, request};
use serde_json::{Value, json};
fn fixture() -> Value {
    let previous = json!({"port":16390,"wsl":{"path":"D:/wsl.exe","sha256":"a".repeat(64)},"distribution":"Ubuntu-24.04","pid":610000,"started":"100","executable":"/usr/bin/redis-check-rdb","sha256":"b".repeat(64),"run_id":"a".repeat(40),"configuration":{"path":"D:/data/redis.conf","sha256":"c".repeat(64)}});
    let markers = json!({"namespace":"ryframe:{scope-seed}:","ownership_key":"ryframe:{scope-seed}:.ryframe-owner","ownership_value":"owned","sentinel_key":"ryframe:devex-fresh:scope-seed:sentinel","sentinel_value":"devex-fresh:fixture"});
    let identity = json!({"pid":previous["pid"],"started":previous["started"],"executable":previous["executable"]});
    let mut process = previous.clone();
    process.as_object_mut().unwrap().remove("pid");
    process.as_object_mut().unwrap().remove("started");
    process.as_object_mut().unwrap().remove("run_id");
    process["scope_id"] = json!("cache-service");
    process["launcher"] = json!("/usr/bin/redis-server");
    process["previous_identity"] = identity.clone();
    process["previous_boot_id"] = json!("boot");
    process["previous_run_id"] = previous["run_id"].clone();
    process["password"] = json!("fixture-secret");
    process["python"] = json!({"path":"/usr/bin/python3","executable":"/usr/bin/python3.12","sha256":"e".repeat(64)});
    let manifest = json!({"path":"manifest.json"});
    let value = json!({"initialized":{"path":"initial.json"},"target_environment":{"path":"environment.json"}});
    let request = json!({"format_version":1,"kind":"devex-clone-cache-restart","side":"target","manifest":manifest,"initialized":value["initialized"],"target_environment":value["target_environment"],"previous":previous,"process_receipt":{"path":"process.json"},"process":process,"markers":markers});
    let mut linux = identity;
    linux["boot_id"] = json!("boot");
    json!({"request":request,"manifest":manifest,"value":value,"initial":{"redis":markers::images(&markers).1,"generation":{"storage":{"redis":previous}}},"original":{"id":"fixture","storage":{"redis":previous},"target":{"scope_id":"scope-seed"},"reset":{"sentinel_key":markers["sentinel_key"],"sentinel_value":markers["sentinel_value"]}},"selected":{"redis":{"namespace":markers["namespace"],"ownership_key":markers["ownership_key"],"ownership_value":markers["ownership_value"],"url":"redis://127.0.0.1:16390/0"}},"review":{"services":{"redis":{"directory":"D:/data","scope_id":"cache-service"}},"tools":{"redis_server":{"path":"/usr/bin/redis-server"}}},"receipt":{"format_version":1,"role":"redis","lifecycle":"running","request_storage":previous,"scope_id":"cache-service","linux_identity":linux},"directory":"D:/data","config":"D:/data/redis.conf","options":[["requirepass","fixture-secret"]]})
}
fn select(value: &Value, fields: &[&str]) -> Value {
    let mut result = json!({});
    for key in fields {
        result[key] = value[key].clone();
    }
    result
}
#[test]
fn request_head_is_exact_and_uses_fixed_manifest_target_and_environment() {
    let a = select(&fixture(), &["request", "manifest", "value"]);
    request::run("request-head", &a).unwrap();
    for (key, value) in [
        ("side", json!("source")),
        ("format_version", json!(true)),
        ("initialized", json!({})),
        ("target_environment", json!({})),
        ("manifest", json!({})),
        ("extra", json!("hidden-secret")),
    ] {
        let mut wrong = a.clone();
        wrong["request"][key] = value;
        let error = request::run("request-head", &wrong).unwrap_err();
        assert!(!error.to_string().contains("hidden-secret"));
    }
}
#[test]
fn request_model_keeps_original_generation_and_exact_two_markers() {
    let a = select(&fixture(), &["request", "initial", "original", "selected"]);
    request::run("request-model", &a).unwrap();
    for field in ["side", "previous", "markers"] {
        let mut wrong = a.clone();
        if field == "side" {
            continue;
        }
        wrong["request"][field] = json!({});
        assert!(request::run("request-model", &wrong).is_err());
    }
    for field in [
        "namespace",
        "ownership_key",
        "sentinel_key",
        "sentinel_value",
    ] {
        let mut wrong = a.clone();
        wrong["request"]["markers"][field] = json!("other");
        assert!(request::run("request-model", &wrong).is_err());
    }
    let mut wrong = a;
    wrong["selected"]["redis"]["url"] = json!("redis://127.0.0.1:16391/0");
    assert!(request::run("request-model", &wrong).is_err());
}
#[test]
fn environment_only_selects_test_and_rejects_all_legacy_extra_fields() {
    assert_eq!(
        request::run(
            "environment-select",
            &json!({"private":{"environment":{"APP_ENV":"test"}}})
        )
        .unwrap(),
        json!({"APP_ENV":"test"})
    );
    for value in [
        json!({"APP_ENV":"test","APP_REDIS_PASSWORD":"private-secret"}),
        json!({"APP_ENV":"test","password_env":"ignored"}),
        json!({"APP_ENV":"dev"}),
        json!({"APP_ENV":true}),
        json!({}),
    ] {
        let error = request::run(
            "environment-select",
            &json!({"private":{"environment":value}}),
        )
        .unwrap_err();
        assert!(!error.to_string().contains("private-secret"));
    }
}
#[test]
fn file_configuration_checks_exact_endpoint_types_and_password_without_echo() {
    let f = fixture();
    let config = json!({"host":"127.0.0.1","port":16390,"database":0,"tls":false,"password":"fixture-secret"});
    request::run(
        "environment-model",
        &json!({"request":f["request"],"redis":config}),
    )
    .unwrap();
    for (key, value) in [
        ("port", json!(true)),
        ("database", json!(false)),
        ("tls", json!(0)),
        ("password", json!("")),
        ("password", json!(true)),
        ("password", Value::Null),
        ("password", json!("\0hidden-secret")),
        ("password", json!("other-secret")),
    ] {
        let mut changed = config.clone();
        changed[key] = value;
        let error = request::run(
            "environment-model",
            &json!({"request":f["request"],"redis":changed}),
        )
        .unwrap_err();
        assert!(!error.to_string().contains("hidden-secret"));
        assert!(!error.to_string().contains("other-secret"));
    }
}
#[test]
fn process_binding_preserves_physical_directory_endpoint_and_original_identity() {
    let a = select(
        &fixture(),
        &[
            "request",
            "review",
            "receipt",
            "directory",
            "config",
            "options",
        ],
    );
    request::run("process-model", &a).unwrap();
    for (key, value) in [
        ("executable", json!("/usr/bin/other")),
        ("port", json!(16391)),
        ("scope_id", json!("other")),
        ("previous_run_id", json!("9".repeat(40))),
    ] {
        let mut wrong = a.clone();
        wrong["request"]["process"][key] = value;
        assert!(request::run("process-model", &wrong).is_err());
    }
    for (key, value) in [
        ("directory", json!("D:/other")),
        ("config", json!("D:/data/other.conf")),
        ("options", json!([["requirepass", "other-secret"]])),
        (
            "options",
            json!([
                ["requirepass", "fixture-secret"],
                ["requirepass", "fixture-secret"]
            ]),
        ),
    ] {
        let mut wrong = a.clone();
        wrong[key] = value;
        let error = request::run("process-model", &wrong).unwrap_err();
        assert!(!error.to_string().contains("other-secret"));
    }
}
fn successor() -> Value {
    let f = fixture();
    let original = &f["request"]["process"];
    json!({"request":f["request"],"directory":"D:/run","source_directory":"D:/run","value":f["value"],"source_manifest":f["value"],"tools":{"wsl":{"path":"D:/wsl.exe","sha256":"d".repeat(64)},"redis_server":{"distribution":original["distribution"],"resolved_path":original["executable"],"sha256":original["sha256"]},"redis_python":{"distribution":original["distribution"],"path":original["python"]["path"],"resolved_path":original["python"]["executable"],"sha256":"f".repeat(64)}},"tool_wsl_path":"D:/wsl.exe","original_wsl_path":"D:/wsl.exe"})
}
#[test]
fn successor_changes_only_explicit_tool_bytes_and_preserves_original() {
    let a = successor();
    let original = a.clone();
    let actual = request::run("successor-model", &a).unwrap();
    let mut expected = a["request"]["process"].clone();
    expected["wsl"] = a["tools"]["wsl"].clone();
    expected["python"]["sha256"] = json!("f".repeat(64));
    assert_eq!(actual, expected);
    assert_eq!(a, original);
}
#[test]
fn successor_rejects_other_run_resources_or_tool_entry() {
    let a = successor();
    for (key, value) in [
        ("distribution", json!("other")),
        ("resolved_path", json!("/usr/bin/other")),
        ("sha256", json!("z".repeat(64))),
    ] {
        let mut wrong = a.clone();
        wrong["tools"]["redis_server"][key] = value;
        assert!(request::run("successor-model", &wrong).is_err());
    }
    for field in ["source_directory", "tool_wsl_path"] {
        let mut wrong = a.clone();
        wrong[field] = json!("D:/other");
        assert!(request::run("successor-model", &wrong).is_err());
    }
    assert!(
        request::run(
            "pending-target",
            &json!({"target":{},"seed_target":{"id":"other"}})
        )
        .is_err()
    );
}
