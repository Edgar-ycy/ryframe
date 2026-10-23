use super::*;
use crate::recovery::clone::native::resources::common;
use serde_json::json;
use std::{fs, path::Path};

fn fixture() -> (tempfile::TempDir, Value) {
    let backend = crate::workspace::root_dir();
    let base = backend.join(".local-tests/source-runtime-complete");
    fs::create_dir_all(&base).unwrap();
    let temp = tempfile::tempdir_in(base).unwrap();
    let directory = temp.path().join("verification");
    fs::create_dir(&directory).unwrap();
    (temp, json!({"backend":backend,"directory":directory}))
}

fn cache_fixture() -> (Value, Value) {
    let subjects = json!(
        (0..11)
            .map(|index| json!({"tenant_id":format!("tenant-{index}"),
        "user_id":format!("{}",9007199254740993_u64+index),"user_authorization_version":1}))
            .collect::<Vec<_>>()
    );
    let plan = super::super::super::model::cache_plan(
        &json!({"namespace":"ryframe:{source-unit}:","subjects":subjects}),
    )
    .unwrap();
    let payloads = subjects.as_array().unwrap().iter().enumerate().map(|(index, subject)| {
        let payload = json!({"versions":{"tenant_authorization_epoch":2,"user_authorization_version":1},"tenant_session_version":1,
            "principal":{"actor":{"user_id":subject["user_id"],"tenant_id":subject["tenant_id"],"username":"owner","dept_id":null,
                "dept_path":null,"data_scope":"all","custom_dept_ids":[],"include_self":true,"is_super_admin":false},
                "tenant_authorization_epoch":2,"preferred_locale":null,"roles":[],"role_ids":[],"permissions":[],"tenant_request_limit_per_minute":600}}).to_string();
        let absent = index % 2 == 0;
        json!({"types":{"tenant_epoch":"string","user_version":"string","snapshot":if absent {"none"} else {"hash"}},
            "tenant_epoch":"2","user_version":"1","fields":if absent {json!([])} else {json!(["2:1"])},
            "payload":if absent {"".to_owned()} else {payload}})
    }).collect::<Vec<_>>();
    let observed = cache_io::observation(&json!({"plan":plan,"raw":payloads})).unwrap();
    (
        json!({"namespace":"ryframe:{source-unit}:","subjects":subjects,"plan":plan,"before":observed["before"]}),
        json!({"raw":payloads,"comparisons":observed["comparisons"]}),
    )
}

#[test]
fn cache_observation_and_cas_bind_exact_role_order_versions_and_payload_bytes() {
    let (observation, data) = cache_fixture();
    let args = json!({"observation":observation,"comparisons":data["comparisons"]});
    let command = cache_io::command(&args).unwrap();
    let command = command.as_array().unwrap();
    assert_eq!(command.len(), 3 + 33 + 55);
    assert_eq!(command[0], "EVAL");
    assert!(
        command[1]
            .as_str()
            .unwrap()
            .contains("redis.call('UNLINK', unpack(KEYS))")
    );
    assert_eq!(command[2], "33");
    assert_eq!(command[3], observation["plan"][0]["keys"]["tenant_epoch"]);
    assert_eq!(command[4], observation["plan"][0]["keys"]["user_version"]);
    assert_eq!(command[5], observation["plan"][0]["keys"]["snapshot"]);
    for (pointer, changed) in [
        ("/comparisons/0", json!("3")),
        ("/comparisons/1", json!("2")),
        ("/comparisons/2", json!("hash")),
        ("/comparisons/3", json!("unexpected")),
        ("/comparisons/9", json!("private-secret-marker")),
    ] {
        let mut invalid = args.clone();
        *invalid.pointer_mut(pointer).unwrap() = changed;
        let error = cache_io::command(&invalid).unwrap_err().to_string();
        assert!(!error.contains("private-secret-marker"));
    }
    let mut invalid = args;
    invalid["comparisons"].as_array_mut().unwrap().pop();
    assert!(cache_io::command(&invalid).is_err());
}

#[test]
fn cache_raw_reads_reject_missing_persistent_keys_versions_fields_and_foreign_subjects() {
    let (observation, data) = cache_fixture();
    let args = json!({"plan":observation["plan"],"raw":data["raw"]});
    for (pointer, changed) in [
        ("/raw/0/types/tenant_epoch", json!("none")),
        ("/raw/0/types/user_version", json!("hash")),
        ("/raw/0/types/snapshot", json!("string")),
        ("/raw/0/tenant_epoch", json!("0")),
        ("/raw/0/tenant_epoch", json!("01")),
        ("/raw/0/user_version", json!("2")),
        ("/raw/0/payload", json!("unexpected")),
        ("/raw/1/fields", json!(["2:1", "other"])),
        ("/raw/1/payload", Value::Null),
        ("/raw/1/payload", json!("{}")),
        ("/plan/1/user_id", json!("foreign")),
    ] {
        let mut invalid = args.clone();
        *invalid.pointer_mut(pointer).unwrap() = changed;
        assert!(cache_io::observation(&invalid).is_err(), "{pointer}");
    }
    let mut invalid = args;
    invalid["raw"].as_array_mut().unwrap().pop();
    assert!(cache_io::observation(&invalid).is_err());
}

#[test]
fn cleanup_result_rejects_loose_scalars_without_claiming_unknown_write_success() {
    assert_eq!(
        cache_io::result(&json!({"value":[27,["expired"]]})).unwrap(),
        json!({"deleted":27,"expired":["expired"]})
    );
    for value in [
        Value::Null,
        json!([true, []]),
        json!([-1, []]),
        json!(["33", []]),
        json!([33, [false]]),
        json!([33, [], "extra"]),
        json!([33, {}]),
    ] {
        assert!(cache_io::result(&json!({"value":value})).is_err());
    }
}

#[test]
fn query_plans_use_safe_identifiers_exact_ranges_and_lossless_string_ids() {
    for (selection, expected) in [
        ("old", "WHERE `id` <= 9007199254740993"),
        ("new", "WHERE `id` > 9007199254740993"),
    ] {
        let sql = operations::query(
            &json!({"database":"control","selection":selection,"upper_id":9007199254740993_u64}),
        )
        .unwrap();
        assert!(sql.as_str().unwrap().contains(expected));
    }
    for (database, selection, upper) in [
        ("control`;DROP", "all", Value::Null),
        ("control", "unknown", Value::Null),
        ("control", "new", json!(true)),
        ("control", "all", json!(1)),
    ] {
        assert!(
            operations::query(&json!({"database":database,"selection":selection,"upper_id":upper}))
                .is_err()
        );
    }
    assert_eq!(
        operations::rows(&json!({"raw":""})).unwrap(),
        json!({"hex":"","upper_id":0})
    );
    let raw = "9007199254740993\t31\t31\t31\tNULL\tNULL\tNULL\t31\tNULL\ttime";
    let value = operations::rows(&json!({"raw":raw})).unwrap();
    assert_eq!(value["upper_id"], 9007199254740993_u64);
    assert_eq!(
        hex::decode(value["hex"].as_str().unwrap()).unwrap(),
        format!("{raw}\n").as_bytes()
    );
}

#[test]
fn runtime_plans_fail_closed_for_ambiguous_control_database_lineage_and_images() {
    let database = json!({"key":"shared-control","kind":"combined","database":"control"});
    assert_eq!(
        operations::control_database(&json!({"databases":[database]})).unwrap(),
        database
    );
    for databases in [
        json!([]),
        json!([database, database]),
        json!([{"key":"shared-control","kind":"business"}]),
    ] {
        assert!(operations::control_database(&json!({"databases":databases})).is_err());
    }
    assert!(operations::image(&json!({"before":{},"expected":{"changed":true}})).is_err());
    let mut args = json!({"directory":"generation/verification","generation":"generation","receipt":{"dataset_lineage":{"a":1}},"lineage":{"a":1}});
    operations::facts_binding(&args).unwrap();
    args["lineage"] = json!({"other":true});
    assert!(operations::facts_binding(&args).is_err());
    args["lineage"] = json!({"a":1});
    args["directory"] = json!("other/verification");
    assert!(operations::facts_binding(&args).is_err());
    assert!(recovery_create::published(&json!({"expected":{},"actual":{"changed":true}})).is_err());
}

fn failed_fixture() -> (tempfile::TempDir, Value) {
    let (temp, args) = fixture();
    let directory = Path::new(args["directory"].as_str().unwrap());
    for name in recovery_create::failed_entries(&json!({}))
        .unwrap()
        .as_array()
        .unwrap()
    {
        let name = name.as_str().unwrap();
        let path = directory.join(name);
        if ["before", "audit", "source-tools"].contains(&name) {
            fs::create_dir(&path).unwrap();
            fs::write(path.join("fixture"), b"evidence").unwrap();
        } else {
            fs::write(path, b"{}").unwrap();
        }
    }
    fs::write(
        directory.join("failed.json"),
        br#"{"status":"failed","error":"ValueError"}"#,
    )
    .unwrap();
    (temp, args)
}

#[test]
fn recovery_prefix_accepts_only_three_explicit_shapes_and_original_value_error() {
    let (_temp, args) = failed_fixture();
    let directory = Path::new(args["directory"].as_str().unwrap());
    recovery_create::prefix(&args).unwrap();
    fs::write(
        directory.join("source-verification-recovery-intent.json"),
        b"{}",
    )
    .unwrap();
    recovery_create::prefix(&args).unwrap();
    fs::create_dir(directory.join("recovery-audit")).unwrap();
    recovery_create::prefix(&args).unwrap();
    for name in [
        "after",
        "cache-cleanup.json",
        "source-runtime.json",
        "unknown",
    ] {
        fs::write(directory.join(name), b"{}").unwrap();
        assert!(recovery_create::prefix(&args).is_err(), "{name}");
        fs::remove_file(directory.join(name)).unwrap();
    }
    fs::write(
        directory.join("failed.json"),
        br#"{"status":"failed","error":"TimeoutExpired"}"#,
    )
    .unwrap();
    assert!(recovery_create::prefix(&args).is_err());
}

#[test]
fn paths_are_preflight_only_and_never_create_output_or_accept_external_paths() {
    let (_temp, args) = fixture();
    let directory = Path::new(args["directory"].as_str().unwrap());
    let start = directory.parent().unwrap().join("start.json");
    fs::write(&start, b"{}").unwrap();
    let mut input = json!({"backend":args["backend"],"start":start,"output":directory.join("source-runtime.json")});
    operations::paths(&input).unwrap();
    assert!(!directory.join("source-runtime.json").exists());
    for output in [
        directory.join("other.json"),
        directory.parent().unwrap().join("source-runtime.json"),
        crate::workspace::root_dir().join("outside/verification/source-runtime.json"),
    ] {
        input["output"] = json!(output);
        assert!(operations::paths(&input).is_err());
    }
}

#[test]
fn login_recheck_preserves_exact_bytes_and_rejects_unknown_write_or_filename() {
    let (_temp, args) = fixture();
    let directory = Path::new(args["directory"].as_str().unwrap());
    fs::create_dir(directory.join("audit")).unwrap();
    let path = directory.join("audit/login-after-old.tsv");
    fs::write(&path, b"original\n").unwrap();
    let mut input = json!({"backend":args["backend"],"directory":args["directory"],"filename":"login-after-old.tsv","raw":"original"});
    let before = common::binding(&path).unwrap();
    audit::recheck(&input).unwrap();
    input["raw"] = json!("new unregistered login");
    assert!(audit::recheck(&input).is_err());
    input["filename"] = json!("../secret");
    assert!(audit::recheck(&input).is_err());
    assert_eq!(common::binding(&path).unwrap(), before);
}

#[test]
fn audit_creation_and_stored_validation_recompute_all_eleven_subjects_and_raw_files() {
    let (_temp, args) = fixture();
    let directory = Path::new(args["directory"].as_str().unwrap());
    fs::create_dir(directory.join("audit")).unwrap();
    let (observation, _) = cache_fixture();
    let tenants = observation["subjects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|subject| json!({"tenant_id":subject["tenant_id"],"username":"owner"}))
        .collect::<Vec<_>>();
    let raw = tenants
        .iter()
        .enumerate()
        .map(|(index, tenant)| {
            let fields = [
                json!(index + 11).to_string(),
                hex::encode_upper(tenant["tenant_id"].as_str().unwrap()),
                hex::encode_upper("owner"),
                hex::encode_upper(format!("198.18.20.{}", index + 1)),
                "NULL".into(),
                "NULL".into(),
                "NULL".into(),
                "31".into(),
                "NULL".into(),
                "2026-01-01T00:00:01.000000".into(),
            ];
            fields.join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n");
    for name in ["login-before.tsv", "login-after-old.tsv"] {
        fs::write(directory.join("audit").join(name), b"old\n").unwrap();
    }
    fs::write(
        directory.join("audit/login-after-new.tsv"),
        format!("{raw}\n"),
    )
    .unwrap();
    let create_args = json!({"backend":args["backend"],"directory":args["directory"],"old_before":hex::encode(b"old\n"),
        "old_after":hex::encode(b"old\n"),"upper_id":10,"new_raw":raw,"lineage":{"tenants":tenants},"subjects":observation["subjects"],
        "started_at":"2026-01-01T00:00:00Z","completed_at":"2026-01-01T00:00:02Z"});
    let value = audit::create(&create_args).unwrap();
    let path = directory.join("audit/login-audit.json");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut stored_args = json!({"backend":args["backend"],"directory":args["directory"],"descriptor":common::binding(&path).unwrap(),
        "lineage":create_args["lineage"],"subjects":create_args["subjects"],"started_at":create_args["started_at"]});
    assert_eq!(audit::stored(&stored_args).unwrap(), value);
    let mut changed = value;
    changed["verified"]["upper_id"] = json!("11");
    fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
    stored_args["descriptor"] = common::binding(&path).unwrap();
    assert!(audit::stored(&stored_args).is_err());
    fs::write(directory.join("audit/login-after-old.tsv"), b"new\n").unwrap();
    assert!(audit::stored(&stored_args).is_err());
}

#[test]
fn recovery_creation_binds_failure_producer_manifest_cache_and_unknown_history() {
    let (_temp, args) = failed_fixture();
    let directory = Path::new(args["directory"].as_str().unwrap());
    fs::write(directory.join("audit/login-audit.json"), b"{}").unwrap();
    let (observation, _) = cache_fixture();
    let before = directory.join("before/image.json");
    fs::write(&before, b"{}").unwrap();
    let mut create_args = json!({"backend":args["backend"],"directory":args["directory"],"start":{"sha256":"a".repeat(64)},
        "lineage":{"sha256":"b".repeat(64)},"before":common::binding(&before).unwrap(),"successor":{"snapshot":{"clean":true}},
        "pre_manifest":super::super::super::inventory::manifest(directory).unwrap(),"authorization_cache":observation,
        "started_at":"2026-01-01T00:00:00Z"});
    let intent = recovery_create::intent(&create_args).unwrap();
    assert_eq!(
        intent["failure"],
        common::binding(&directory.join("failed.json")).unwrap()
    );
    create_args["pre_manifest"] = json!([]);
    assert!(recovery_create::intent(&create_args).is_err());
    let intent_path = directory.join("source-verification-recovery-intent.json");
    fs::write(&intent_path, serde_json::to_vec(&intent).unwrap()).unwrap();
    assert!(recovery_create::intent(&create_args).is_err());
    let keys = cache_io::keys(&json!({"plan":observation["plan"]})).unwrap();
    let expired = observation["before"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["snapshot"]["type"] == "none")
        .map(|row| row["snapshot"]["key"].clone())
        .collect::<Vec<_>>();
    let cleanup = recovery::cache_publication(&json!({"observation":observation,"deleted":null,"expired":expired,
        "execution":"reconciled_unknown_lua_result","after":keys.as_array().unwrap().iter().map(|key| json!({"key":key,"type":"none"})).collect::<Vec<_>>(),
        "cleaned_at":"2026-01-01T00:00:01Z"})).unwrap();
    fs::write(
        directory.join("cache-cleanup.json"),
        serde_json::to_vec(&cleanup).unwrap(),
    )
    .unwrap();
    fs::create_dir(directory.join("recovery-audit")).unwrap();
    fs::write(directory.join("recovery-audit/check.json"), b"{}").unwrap();
    let receipt_args = json!({"backend":args["backend"],"directory":args["directory"],"cleanup":cleanup,
        "after":{"sha256":"c".repeat(64)},"completed_at":"2026-01-01T00:00:02Z"});
    let receipt = recovery_create::receipt(&receipt_args).unwrap();
    assert_eq!(receipt["historical_cleanup"], "unknown");
    assert_eq!(
        receipt["confirmed_cleanup"],
        "reconciled_unknown_lua_result_all_keys_absent"
    );
    assert_eq!(receipt["intent"], common::binding(&intent_path).unwrap());
    fs::write(directory.join("cache-cleanup.json"), b"{}").unwrap();
    assert!(recovery_create::receipt(&receipt_args).is_err());
}
