use super::{clone, common, source};
use serde_json::{Value, json};
use std::{ffi::OsString, path::Path};

#[test]
fn tracked_openapi_snapshot_stays_bound_without_relaxing_managed_inputs() {
    let local = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let fixture = tempfile::tempdir_in(local).unwrap();
    let root = crate::recovery::fixture_control::portable_path(fixture.path());
    std::fs::create_dir_all(root.join(".local-tests")).unwrap();
    std::fs::create_dir_all(root.join("openapi")).unwrap();
    let path = root.join("openapi/openapi.json");
    std::fs::write(&path, br#"{"openapi":"3.1.0"}"#).unwrap();
    assert!(common::Snapshot::read(&root, &path, 1024, "受管计划").is_err());
    let (snapshot, contract) = common::Snapshot::openapi(&root).unwrap();
    assert_eq!(contract["openapi"], "3.1.0");
    snapshot.unchanged(&root).unwrap();
    std::fs::write(&path, br#"{"openapi":"3.0.0"}"#).unwrap();
    assert!(snapshot.unchanged(&root).is_err());
}

fn descriptor(root: &Path, name: &str) -> Value {
    json!({"path":root.join(name),"bytes":1,"sha256":"a".repeat(64)})
}

fn lineage() -> Value {
    let root = crate::workspace::root_dir().join(".local-tests/existing-native-model");
    let mut next_file = 1u64;
    let tenants = (0..11)
        .map(|index| {
            let count = 256 / 11 + usize::from(index < 256 % 11);
            let tenant = if index == 0 { "system".into() } else { format!("origin-{index:02}") };
            let files = (0..count).map(|_| {
                let id = next_file;
                next_file += 1;
                json!({"file_id":id.to_string(),"file_name":format!("{id}.bin"),"file_path":format!("{tenant}/{id}.bin"),"file_url":format!("/files/{id}"),"bytes":4194304,"sha256":"b".repeat(64)})
            }).collect::<Vec<_>>();
            json!({"tenant_id":tenant,"database":"shared","username":if index == 0 {"admin"} else {"owner"},"records":if index == 0 {9090} else {9091},"posts":[{"id":"1","code":"one","name":"一"},{"id":"2","code":"two","name":"二"},{"id":"3","code":"three","name":"三"}],"files":files})
        })
        .collect::<Vec<_>>();
    let mut value = json!({
        "format_version":1,"kind":"restore-source-derived-dataset-lineage","status":"derived_dataset_verified",
        "scopes":{"origin_tenant_scope_id":"origin","current_source_scope_id":"current","current_object_scope_id":"current"},
        "scale":{"records":100000,"current_post_rows":100000,"tenants":11,"post_samples":33,"business_objects":256,"verified_objects":257,"object_bytes":1073741824u64},
        "tenants":tenants,
        "objects":{"business_objects":256,"business_bytes":1073741824u64,"mapping_sha256":"c".repeat(64),"verified_objects":257,"probe":{"bucket":"uploads","source_key":"origin/probe","target_key":"current/probe","bytes":1,"sha256":"d".repeat(64),"metadata":{}}},
        "verification":{"scope_id":"current","api_url":"http://127.0.0.1:3100","frontend_url":"http://127.0.0.1:4100","request_interval_ms":1000},
        "restore_qualified":false,
    });
    for field in super::source::DESCRIPTORS {
        value[*field] = descriptor(&root, field);
    }
    value
}

#[test]
fn source_lineage_preserves_full_scale_and_rejects_scope_or_aggregate_drift() {
    let valid = lineage();
    source::validate_lineage(&valid).unwrap();
    let mutations: [fn(&mut Value); 5] = [
        |value: &mut Value| value["scopes"]["current_source_scope_id"] = json!("other"),
        |value: &mut Value| value["scale"]["records"] = json!(99_999),
        |value: &mut Value| value["tenants"][1]["tenant_id"] = json!("current-01"),
        |value: &mut Value| value["objects"]["probe"]["target_key"] = json!("other/probe"),
        |value: &mut Value| value["objects"]["probe"]["target_key"] = json!("current/other"),
    ];
    for mutation in mutations {
        let mut invalid = valid.clone();
        mutation(&mut invalid);
        assert!(source::validate_lineage(&invalid).is_err());
    }
    let mut invalid = valid;
    invalid["tenants"][0]["secret_key"] = json!("must-not-leak");
    let error = source::validate_lineage(&invalid).unwrap_err().to_string();
    assert!(!error.contains("must-not-leak"));
}

#[test]
fn source_lineage_probe_presence_and_counts_must_agree() {
    let historical = lineage();
    let mut current = historical.clone();
    current["scale"]["verified_objects"] = json!(256);
    current["objects"]["verified_objects"] = json!(256);
    current["objects"]["probe"] = Value::Null;
    source::validate_lineage(&current).unwrap();

    for (section, count) in [("scale", 257), ("objects", 257), ("objects", 255)] {
        let mut invalid = current.clone();
        invalid[section]["verified_objects"] = json!(count);
        assert!(source::validate_lineage(&invalid).is_err());
    }
    let mut invalid = current.clone();
    invalid["objects"]["probe"] = historical["objects"]["probe"].clone();
    assert!(source::validate_lineage(&invalid).is_err());
    let mut invalid = historical.clone();
    invalid["objects"]["probe"] = Value::Null;
    assert!(source::validate_lineage(&invalid).is_err());
    let mut invalid = historical.clone();
    invalid["objects"]["probe"] = json!([
        historical["objects"]["probe"],
        historical["objects"]["probe"]
    ]);
    assert!(source::validate_lineage(&invalid).is_err());
    let mut invalid = current.clone();
    invalid["objects"]["business_objects"] = json!(255);
    assert!(source::validate_lineage(&invalid).is_err());
    let mut invalid = current;
    invalid["objects"].as_object_mut().unwrap().remove("probe");
    assert!(source::validate_lineage(&invalid).is_err());
}

#[test]
fn clone_target_requires_original_plan_and_complete_copy_evidence() {
    let plan = json!({"source":{"scope_id":"origin"}});
    let valid = json!({
        "format_version":1,"kind":"devex-copy-business-target",
        "source_plan_sha256":super::super::model::hash(&plan).unwrap(),"source_scope_id":"origin",
        "target":{"scope_id":"seed-copy","api_url":"http://127.0.0.1:3200","frontend_url":"http://127.0.0.1:4200"},
        "copy":{"plan_sha256":"a".repeat(64),"generation_sha256":"b".repeat(64),"source_export_sha256":"c".repeat(64),"fresh_target_sha256":"d".repeat(64),"stage_receipt_sha256":"e".repeat(64),"ledger_head_sha256":"f".repeat(64)},
    });
    clone::validate_target(&plan, &valid).unwrap();
    for field in ["generation_sha256", "ledger_head_sha256"] {
        let mut invalid = valid.clone();
        invalid["copy"][field] = json!("invalid");
        assert!(clone::validate_target(&plan, &invalid).is_err());
    }
    let mut invalid = valid;
    invalid["target"]["api_url"] = json!("https://example.com");
    assert!(clone::validate_target(&plan, &invalid).is_err());
}

#[test]
fn private_arguments_require_unique_complete_non_secret_values() {
    let required = ["--backend-dir", "--run-dir"];
    let valid = [
        "--backend-dir",
        "D:/backend",
        "--run-dir",
        "D:/backend/.local-tests/run",
        "--write",
    ]
    .map(OsString::from);
    assert!(common::arguments(&valid, &required, "--write").is_ok());
    for invalid in [
        vec!["--backend-dir", "D:/backend", "--write"],
        vec![
            "--backend-dir",
            "D:/backend",
            "--backend-dir",
            "D:/other",
            "--run-dir",
            "D:/run",
            "--write",
        ],
        vec![
            "--backend-dir",
            "private-password",
            "--run-dir",
            "D:/run",
            "--unknown",
        ],
    ] {
        let args = invalid.into_iter().map(OsString::from).collect::<Vec<_>>();
        let error = common::arguments(&args, &required, "--write")
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private-password"));
    }
}

#[test]
fn start_gate_accepts_only_one_exact_authorization_and_never_echoes_input() {
    let expected = json!({"operation":"start","run_dir":"D:/run","attempt":7});
    common::read_start(serde_json::to_vec(&expected).unwrap().as_slice(), &expected).unwrap();
    for invalid in [
        Vec::new(),
        b"{}".to_vec(),
        b"{\"operation\":\"start\"}\n{}".to_vec(),
        vec![b'x'; 4097],
        b"private-start-secret".to_vec(),
    ] {
        let error = common::read_start(invalid.as_slice(), &expected)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private-start-secret"));
    }
}

#[test]
fn shared_source_and_clone_password_loader_uses_current_credentials_without_bootstrap() {
    use crate::recovery::dataset_prepare::model;
    use std::fs;
    let root = crate::workspace::root_dir();
    fs::create_dir_all(root.join(".local-tests")).unwrap();
    let temporary = tempfile::tempdir_in(root.join(".local-tests")).unwrap();
    let config = temporary.path().join("config");
    fs::create_dir(&config).unwrap();
    let bytes = b"scope_id='verify-current-only'\n";
    fs::write(config.join("app.test.toml"), bytes).unwrap();
    let mut binding = json!({"app_env":"test","directory":config,"files":{"app.test.toml":{"bytes":bytes.len(),"sha256":model::sha(bytes)}}});
    binding["sha256"] = json!(model::hash(&binding).unwrap());
    let binding_file = temporary.path().join("configuration.json");
    fs::write(&binding_file, serde_json::to_vec(&binding).unwrap()).unwrap();
    let credentials = temporary.path().join("administrator.toml");
    let bytes =
        b"[administrator]\ntenant_id='system'\nusername='admin'\npassword='Current!Changed123'\n";
    fs::write(&credentials, bytes).unwrap();
    let dataset = json!({"admin":{"tenant_id":"system","username":"admin"},
        "administrator_credentials":{"path":credentials,"bytes":bytes.len(),"sha256":model::sha(bytes)}});
    let mut snapshots = Vec::new();
    let mut configuration_snapshots = Vec::new();
    assert_eq!(
        common::password(
            &root,
            &root,
            &binding_file,
            &dataset,
            &mut snapshots,
            &mut configuration_snapshots
        )
        .unwrap(),
        "Current!Changed123"
    );
    common::unchanged(&root, &snapshots, &configuration_snapshots).unwrap();
    fs::write(&credentials, b"private-drift").unwrap();
    assert!(common::unchanged(&root, &snapshots, &configuration_snapshots).is_err());
}
