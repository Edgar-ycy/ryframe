use super::{batch, model, prepare};
use serde_json::{Value, json};
use std::fs;

fn plan() -> Value {
    let mut value = json!({"id":"restore-case","work_dir":"unused","source":{"scope_id":"source-case","api_url":"http://127.0.0.1:3000","frontend_url":"http://127.0.0.1:4174","databases":[{"key":"shared","mode":"shared"},{"key":"dedicated","mode":"dedicated"}]},"target":{"scope_id":"target-case","api_url":"http://127.0.0.1:3001","frontend_url":"http://127.0.0.1:4175"},"dataset":{"tenant_targets":["shared","shared","shared","shared","shared","shared","shared","shared","shared","dedicated"],"request_interval_ms":1000,"api_validation_posts":2,"post_batch_rows":1000,"records":100_000,"object_count":256,"object_bytes":4194304,"admin":{"tenant_id":"system","username":"admin"},"configuration":{"app_env":"test","directory":"unused","files":{},"sha256":""}}});
    for side in ["source", "target"] {
        value[side]["s3"] = json!({"endpoint":"http://127.0.0.1:9000","region":"us-east-1","configuration":value["dataset"]["configuration"]});
    }
    value
}
#[test]
fn dataset_specification_preserves_scale_targets_and_rejects_inline_credentials() {
    let valid = plan();
    model::specification(&valid).unwrap();
    for (field, value) in [
        ("records", json!(99999)),
        ("records", json!(1_000_001)),
        ("records", json!(true)),
        ("object_count", json!(255)),
        ("object_bytes", json!(4194305)),
        ("request_interval_ms", json!(999)),
        ("request_interval_ms", json!(1000.5)),
        ("post_batch_rows", json!(99)),
        ("api_validation_posts", json!(11)),
    ] {
        let mut invalid = valid.clone();
        invalid["dataset"][field] = value;
        assert!(model::specification(&invalid).is_err(), "{field}");
    }
    let mut invalid = valid.clone();
    invalid["dataset"]["admin"]["password_env"] = json!("OLD_PASSWORD");
    assert!(model::specification(&invalid).is_err());
    invalid = valid.clone();
    invalid["dataset"]["admin"]["password"] = json!("private-password");
    assert!(model::specification(&invalid).is_err());
    invalid = valid.clone();
    invalid["dataset"]["owner_password"] = json!("private-password");
    assert!(model::specification(&invalid).is_err());
    invalid = valid.clone();
    invalid["dataset"]["tenant_targets"][0] = json!("dedicated");
    assert!(model::specification(&invalid).is_err());
    invalid = valid.clone();
    invalid["source"]["api_url"] = json!("https://example.com");
    assert!(model::specification(&invalid).is_err());
    for field in [
        "access_key",
        "secret_key",
        "session_token",
        "security_token",
    ] {
        let mut invalid = valid.clone();
        invalid["source"]["s3"][field] = json!("private-value");
        let error = model::specification(&invalid).unwrap_err().to_string();
        assert!(!error.contains("private-value"));
    }
}
#[test]
fn hashes_and_binary_samples_match_fixed_vectors() {
    assert_eq!(
        model::hash(&json!({"b":2,"a":"中文"})).unwrap(),
        model::sha("{\"a\":\"中文\",\"b\":2}".as_bytes())
    );
    let sample = prepare::sample_content("fixture", 3, 1024);
    assert_eq!(sample.len(), 1024);
    assert_eq!(sample, prepare::sample_content("fixture", 3, 1024));
    assert_ne!(sample, prepare::sample_content("fixture", 4, 1024));
    assert_eq!(
        &sample[..65],
        format!("{}\n", model::sha(b"fixture:3:0")).as_bytes()
    );
}
#[test]
fn preflight_is_bound_to_plan_side_scope_and_empty_source() {
    let plan = plan();
    let valid = json!({"format_version":1,"kind":"restore-reference-dataset-preflight","plan_sha256":model::hash(&plan).unwrap(),"side":"source","scope_id":"source-case","actions":{"business":"empty_source_verified"}});
    model::verify_preflight(&plan, &valid).unwrap();
    for (key, value) in [
        ("side", json!("target")),
        ("scope_id", json!("other")),
        ("plan_sha256", json!("0".repeat(64))),
        ("actions", json!({"business":"unchecked"})),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        assert!(model::verify_preflight(&plan, &invalid).is_err());
    }
}
#[test]
fn batch_distribution_preserves_all_rows_sort_and_i64_ids() {
    let plan = plan();
    let mut total = 0;
    for index in 0..11 {
        let count = model::count(100_000, index);
        total += count;
        let row = batch::row(&plan, index, 1000);
        assert_eq!(row["sort"], 0);
        assert_eq!(row["name"], "恢复样本1000");
        assert_eq!(row["code"], "restore-case-1000");
    }
    assert_eq!(total, 100_000);
    let result = json!({"kind":"restore-reference-post-batch","first_id":"9007199254740993","last_id":"9007199254740995","rows":3});
    let samples =
        [json!({"identityIndex":0,"position":2,"row":{"code":"sample","name":"恢复样本"}})];
    assert_eq!(
        batch::resolve_samples(&samples, &result).unwrap()[0]["id"],
        "9007199254740995"
    );
    let mut invalid = result.clone();
    invalid["last_id"] = json!("1");
    for invalid in [invalid, Value::Null] {
        assert!(batch::resolve_samples(&samples, &invalid).is_err());
    }
}
fn dataset(plan: &Value) -> Value {
    let tenants=(0..11).map(|i|json!({"tenant_id":model::tenant(plan,i),"username":model::identity(plan,i)["username"],"posts":[{"id":"1","code":"sample","name":"恢复样本"}],"files":[]})).collect::<Vec<_>>();
    json!({"format_version":1,"plan_sha256":model::hash(plan).unwrap(),"source_scope_id":plan["source"]["scope_id"],"tenants":tenants})
}
#[test]
fn existing_dataset_rejects_repeated_tenants_invalid_ids_and_files() {
    let plan = plan();
    let valid = dataset(&plan);
    model::validate_dataset(&plan, &valid).unwrap();
    let mut invalid = valid.clone();
    invalid["tenants"][1] = invalid["tenants"][0].clone();
    assert!(model::validate_dataset(&plan, &invalid).is_err());
    invalid = valid.clone();
    invalid["tenants"][0]["posts"][0]["id"] = json!("9223372036854775808");
    assert!(model::validate_dataset(&plan, &invalid).is_err());
    invalid = valid.clone();
    invalid["tenants"][0]["files"] =
        json!([{"file_path":"file","bytes":4194305,"sha256":"a".repeat(64)}]);
    assert!(model::validate_dataset(&plan, &invalid).is_err());
}
fn directory() -> tempfile::TempDir {
    let root = crate::workspace::root_dir().join(".local-tests/dataset-native-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
#[test]
fn protocol_exact_fields_paths_modes_and_input_mutation_fail_closed() {
    let root = crate::workspace::root_dir();
    let temp = directory();
    let mut plan = plan();
    plan["work_dir"] = json!(temp.path());
    plan["dataset"]["configuration"] = configuration(temp.path());
    let plan_path = temp.path().join("plan.json");
    let preflight_path = temp.path().join("preflight.json");
    fs::write(&plan_path, serde_json::to_vec(&plan).unwrap()).unwrap();
    fs::write(&preflight_path,serde_json::to_vec(&json!({"format_version":1,"kind":"restore-reference-dataset-preflight","plan_sha256":model::hash(&plan).unwrap(),"side":"source","scope_id":"source-case","actions":{"business":"empty_source_verified"}})).unwrap()).unwrap();
    let request = json!({"backend_dir":root,"format_version":1,"kind":super::KIND,"plan":plan_path,"preflight":preflight_path,"verify_existing":null,"side":"source","write":true});
    let inputs = model::Inputs::read(&request, &root).unwrap();
    assert_eq!(inputs.password, "fixture-memory-only-password");
    assert!(
        !serde_json::to_string(&plan)
            .unwrap()
            .contains("fixture-memory-only-password")
    );
    inputs.unchanged().unwrap();
    for (key, value) in [
        ("write", json!(false)),
        ("side", json!("target")),
        ("unknown", json!(true)),
        ("plan", json!("relative.json")),
        ("verify_existing", json!(preflight_path)),
    ] {
        let mut invalid = request.clone();
        invalid[key] = value;
        assert!(model::Inputs::read(&invalid, &root).is_err());
    }
    fs::write(preflight_path, b"{}").unwrap();
    assert!(inputs.unchanged().is_err());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
}

fn configuration(directory: &std::path::Path) -> Value {
    let config = directory.join("config");
    fs::create_dir(&config).unwrap();
    let bytes = b"[bootstrap]\nadmin_password='fixture-memory-only-password'\n[object_storage]\naccess_key='fixture-memory-only-access'\nsecret_key='fixture-memory-only-secret'\n";
    fs::write(config.join("app.test.toml"), bytes).unwrap();
    let mut value = json!({"app_env":"test","directory":config,"files":{"app.test.toml":{"bytes":bytes.len(),"sha256":model::sha(bytes)}}});
    value["sha256"] = json!(model::hash(&value).unwrap());
    value
}
#[test]
fn config_binding_loads_password_only_in_memory_and_redacts_failure() {
    let root = crate::workspace::root_dir();
    let temp = directory();
    let binding = configuration(temp.path());
    let mut snapshots = Vec::new();
    assert_eq!(
        super::configuration::password(&root, &binding, &mut snapshots).unwrap(),
        "fixture-memory-only-password"
    );
    assert!(
        !serde_json::to_string(&binding)
            .unwrap()
            .contains("fixture-memory-only-password")
    );
    let objects =
        json!({"endpoint":"http://127.0.0.1:9000","region":"us-east-1","configuration":binding});
    assert!(crate::recovery::clone::native::s3::Configuration::read(&objects).is_err());
    assert!(
        crate::recovery::clone::native::s3::Configuration::read_app_binding(&root, &objects)
            .is_ok()
    );
    for secret in [
        "fixture-memory-only-password",
        "fixture-memory-only-access",
        "fixture-memory-only-secret",
    ] {
        assert!(!serde_json::to_string(&objects).unwrap().contains(secret));
    }
    let file = temp.path().join("config/app.test.toml");
    fs::write(
        &file,
        b"[bootstrap]\nadmin_password='another-memory-only-password'\n",
    )
    .unwrap();
    let error = super::configuration::password(&root, &binding, &mut Vec::new())
        .unwrap_err()
        .to_string();
    assert!(error.contains("摘要"));
    assert!(!error.contains("memory-only-password"));
    let error =
        crate::recovery::clone::native::s3::Configuration::read_app_binding(&root, &objects)
            .err()
            .unwrap()
            .to_string();
    assert!(error.contains("摘要"));
    assert!(!error.contains("memory-only"));
    fs::write(&file, b"[bootstrap]\nadmin_password='private-unclosed\n").unwrap();
    let bytes = fs::read(&file).unwrap();
    let mut invalid = json!({"app_env":"test","directory":file.parent().unwrap(),"files":{"app.test.toml":{"bytes":bytes.len(),"sha256":model::sha(&bytes)}}});
    invalid["sha256"] = json!(model::hash(&invalid).unwrap());
    let error = super::configuration::password(&root, &invalid, &mut Vec::new())
        .unwrap_err()
        .to_string();
    assert!(!error.contains("private-unclosed"));
}
#[test]
fn duplicate_protocol_keys_and_noncanonical_ids_are_rejected() {
    assert!(
        super::documents::parse_fragment("{\"write\":true,\"wr\\u0069te\":true}", "request")
            .is_err()
    );
    for value in ["0", "01", "-1", "1.0", "9223372036854775808"] {
        assert!(model::positive_id(&json!(value)).is_err());
    }
}

#[path = "tests_http.rs"]
mod http;
