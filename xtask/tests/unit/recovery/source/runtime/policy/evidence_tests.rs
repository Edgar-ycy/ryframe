use super::*;
use crate::recovery::clone::native::resources::common;
use serde_json::json;
use std::{fs, path::Path};

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn fixture() -> (tempfile::TempDir, Value, Value, Value) {
    let backend = crate::workspace::root_dir();
    let base = backend.join(".local-tests/source-runtime-policy");
    fs::create_dir_all(&base).unwrap();
    let temp = tempfile::tempdir_in(base).unwrap();
    let directory = temp.path().join("verification");
    fs::create_dir(&directory).unwrap();
    let tool = std::env::current_exe().unwrap();
    let configuration = json!({"environment":"test","files":[]});
    write(
        &directory.join("source-producer-configuration.json"),
        &configuration,
    );
    let identity = json!({"pid":1234,"started":"kernel-start","executable":tool});
    let args = json!({"backend":backend,"directory":directory,"start":{"sha256":"a".repeat(64)},
        "lineage":{"sha256":"b".repeat(64)},"tool":tool,"argv":["native","--write"],
        "environment_sha256":"c".repeat(64),"coordinator_source":{"snapshot":{"clean":true}},
        "execution_sha":"d".repeat(40),"configuration":configuration});
    let intent = json!({"format_version":2,"kind":"restore-source-producer-intent","operation_id":"e".repeat(32),
        "controller":identity,"source_generation":args["start"],"dataset_lineage":args["lineage"],
        "executable":tool,"source":{},"configuration":common::binding(&directory.join("source-producer-configuration.json")).unwrap(),
        "argv":args["argv"],"cwd":backend,"environment_sha256":args["environment_sha256"],
        "started_at":"2026-01-01T00:00:00Z","coordinator_source":args["coordinator_source"],
        "execution_sha":args["execution_sha"],"staging":{},"inputs":[]});
    write(&directory.join("source-producer-intent.json"), &intent);
    let producer = json!({"format_version":1,"kind":"restore-source-producer","operation_id":intent["operation_id"],
        "intent":common::binding(&directory.join("source-producer-intent.json")).unwrap(),"controller":identity,
        "process":identity,"executable":tool,"argv":args["argv"]});
    write(&directory.join("source-producer.json"), &producer);
    (temp, args, intent, producer)
}

#[test]
fn producer_registration_binds_configuration_generation_command_environment_and_identity() {
    let (_temp, args, intent, original) = fixture();
    producer::registration(&args).unwrap();
    let directory = Path::new(args["directory"].as_str().unwrap());
    for (field, changed) in [
        ("format_version", json!(true)),
        ("source_generation", json!({})),
        ("dataset_lineage", json!({})),
        ("executable", json!("other")),
        ("argv", json!([])),
        ("environment_sha256", json!("changed")),
        ("coordinator_source", json!({})),
        ("execution_sha", json!("changed")),
        (
            "controller",
            json!({"pid":true,"started":"start","executable":args["tool"]}),
        ),
        ("started_at", json!("2999-01-01T00:00:00Z")),
        ("operation_id", json!("INVALID")),
    ] {
        let mut changed_intent = intent.clone();
        changed_intent[field] = changed;
        write(
            &directory.join("source-producer-intent.json"),
            &changed_intent,
        );
        let mut process = original.clone();
        process["intent"] =
            common::binding(&directory.join("source-producer-intent.json")).unwrap();
        write(&directory.join("source-producer.json"), &process);
        assert!(producer::registration(&args).is_err(), "{field}");
    }
    write(&directory.join("source-producer-intent.json"), &intent);
    write(&directory.join("source-producer.json"), &original);
    write(
        &directory.join("source-producer-configuration.json"),
        &json!({"changed":true}),
    );
    assert!(producer::registration(&args).is_err());
}

#[test]
fn completion_binds_empty_streams_exact_exit_and_ordered_time() {
    let (_temp, args, intent, process) = fixture();
    let directory = Path::new(args["directory"].as_str().unwrap());
    for file in [
        "source-producer-ready.json",
        "source-producer.stdout.json",
        "source-producer.stderr.log",
    ] {
        fs::write(directory.join(file), b"").unwrap();
    }
    let mut value = json!({"format_version":1,"kind":"restore-source-producer-completion","operation_id":intent["operation_id"],
        "process":process["process"],"exit_code":7,"completed_at":"2026-01-01T00:00:01Z"});
    for (key, name) in [
        ("intent", "source-producer-intent.json"),
        ("producer", "source-producer.json"),
        ("ready", "source-producer-ready.json"),
        ("stdout", "source-producer.stdout.json"),
        ("stderr", "source-producer.stderr.log"),
    ] {
        value[key] = common::binding(&directory.join(name)).unwrap();
    }
    let parameters = json!({"backend":args["backend"],"directory":args["directory"],"intent":intent,"producer":process});
    let path = directory.join("source-producer-completion.json");
    write(&path, &value);
    assert_eq!(producer::completion(&parameters).unwrap()["exit_code"], 7);
    for (field, changed) in [
        ("exit_code", json!(true)),
        ("completed_at", json!("2025-01-01T00:00:00Z")),
        ("stdout", json!({})),
        ("operation_id", json!("f".repeat(32))),
    ] {
        let mut altered = value.clone();
        altered[field] = changed;
        write(&path, &altered);
        assert!(producer::completion(&parameters).is_err(), "{field}");
    }
    write(&path, &value);
    fs::write(directory.join("source-producer.stderr.log"), b"late change").unwrap();
    assert!(producer::completion(&parameters).is_err());
}

#[test]
fn snapshot_identity_rejects_subject_version_and_duplicate_field_changes() {
    let subject =
        json!({"user_id":"9007199254740993","tenant_id":"system","user_authorization_version":1});
    let payload = json!({"versions":{"tenant_authorization_epoch":1,"user_authorization_version":1,"data_scope_version":i64::MAX},"tenant_session_version":1,
        "principal":{"actor":{"user_id":"9007199254740993","tenant_id":"system","username":"owner","dept_id":null,
            "dept_path":null,"data_scope":"all","custom_dept_ids":[],"include_self":true,"data_scope_version":i64::MAX,"is_super_admin":false},
            "password_change_required":false,"user_authorization_version":1,"data_scope_version":i64::MAX,"tenant_authorization_epoch":1,"preferred_locale":null,"roles":[],"role_ids":[],"permissions":[],"tenant_request_limit_per_minute":600}});
    let args = json!({"payload":payload.to_string(),"subject":subject,"tenant_epoch":"1"});
    assert!(model::snapshot(&args).is_ok());
    let mut invalid = args.clone();
    invalid["subject"]["tenant_id"] = json!("other");
    assert!(model::snapshot(&invalid).is_err());
    invalid = args.clone();
    invalid["tenant_epoch"] = json!("2");
    assert!(model::snapshot(&invalid).is_err());
    for (pointer, replacement) in [
        ("/versions/data_scope_version", json!(0)),
        ("/versions/data_scope_version", json!(1.0)),
        (
            "/versions/tenant_authorization_epoch",
            json!(i64::from(i32::MAX) + 1),
        ),
        ("/principal/user_authorization_version", json!(2)),
        ("/principal/data_scope_version", json!(2)),
        ("/principal/actor/data_scope_version", json!(2)),
        ("/principal/password_change_required", json!(true)),
        ("/principal/password_change_required", json!("false")),
        ("/tenant_session_version", json!(false)),
    ] {
        let mut changed = payload.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        let mut invalid = args.clone();
        invalid["payload"] = json!(changed.to_string());
        assert!(model::snapshot(&invalid).is_err(), "{pointer}");
    }
    let mut legacy = payload.clone();
    legacy["versions"]
        .as_object_mut()
        .unwrap()
        .remove("data_scope_version");
    for name in [
        "password_change_required",
        "user_authorization_version",
        "data_scope_version",
    ] {
        legacy["principal"].as_object_mut().unwrap().remove(name);
    }
    legacy["principal"]["actor"]
        .as_object_mut()
        .unwrap()
        .remove("data_scope_version");
    invalid = args.clone();
    invalid["payload"] = json!(legacy.to_string());
    assert!(model::snapshot(&invalid).is_err());
    let mut secret = payload.clone();
    secret["secret"] = json!("private-source-v4-secret");
    invalid = args.clone();
    invalid["payload"] = json!(secret.to_string());
    assert!(
        !model::snapshot(&invalid)
            .unwrap_err()
            .to_string()
            .contains("private-source-v4-secret")
    );
    invalid = args;
    invalid["payload"] = json!("{\"versions\":{},\"versions\":{}}");
    assert!(model::snapshot(&invalid).is_err());
}
