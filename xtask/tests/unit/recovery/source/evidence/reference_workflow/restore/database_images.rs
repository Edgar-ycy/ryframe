use super::*;
use crate::recovery::{fixture_control::portable_path, inputs::domain::TARGET_DATABASES};

fn declared() -> (Value, Value, Value, Value) {
    let scope = "restore-target";
    let databases = TARGET_DATABASES.map(|(key, kind, mode)| {
        json!({"key":key,"kind":kind,"mode":mode,
            "database":format!("db_{key}"),"server_uuid":"server-one"})
    });
    let configuration = json!({"app_env":"test","sha256":"a".repeat(64)});
    let endpoint = "http://127.0.0.1:19001";
    let region = "us-east-1";
    let request = json!({"side":"base","target":{"scope_id":scope,"databases":databases,
        "s3":{"endpoint":endpoint,"region":region,"access_key":"fixture-private","secret_key":"fixture-secret"}}});
    let reference = json!({"target_side":"base","target":{"scope_id":scope,"databases":databases,
        "s3":{"endpoint":endpoint,"region":region,"configuration":configuration},
        "runtime_dir":"fixture-runtime","api_url":"http://127.0.0.1:19002",
        "worker_ready_url":"http://127.0.0.1:19003/readyz","frontend_url":"http://127.0.0.1:19004"}});
    let images = databases
        .iter()
        .map(|database| {
            (
                database["database"].as_str().unwrap(),
                json!({"resource":{
                    "kind":"database","scope_id":scope,
                    "database":database["database"],"server_uuid":database["server_uuid"],
                }}),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let initialized = json!({"status":"fresh_target_initialized","scope_id":scope,
        "controlled_generation_never_started":true,"generation":{"selected":reference["target"]},
        "inventory":{"observations":images}});
    (request, reference, initialized, configuration)
}

#[test]
fn database_capture_input_binds_original_scope_placement_configuration_and_physical_graph() {
    let (request, reference, initialized, configuration) = declared();
    let environment = BTreeMap::new();
    let backend = crate::workspace::root_dir();
    let storage = StorageInputs {
        backend: &backend,
        request: &request,
        environment: &environment,
        observation_dir: &backend,
    };
    let selected = selected_target(&storage, &reference, &initialized, &configuration).unwrap();
    assert_eq!(selected["databases"], request["target"]["databases"]);
    assert_eq!(selected["s3"], json!({"configuration":configuration}));
    assert_eq!(selected["runtime_dir"], reference["target"]["runtime_dir"]);
    for (field, value) in [
        ("scope_id", json!("another-target")),
        ("worker_ready_url", json!("http://127.0.0.1:19005/readyz")),
        ("databases", json!([])),
    ] {
        let mut changed = reference.clone();
        changed["target"][field] = value;
        assert!(selected_target(&storage, &changed, &initialized, &configuration).is_err());
    }
    let mut changed = initialized.clone();
    changed["inventory"]["observations"]["db_shared"]["resource"]["server_uuid"] =
        json!("another-server");
    assert!(selected_target(&storage, &reference, &changed, &configuration).is_err());
    let mut changed = configuration;
    changed["sha256"] = json!("b".repeat(64));
    assert!(selected_target(&storage, &reference, &initialized, &changed).is_err());
}

#[test]
fn post_capture_tree_recheck_rejects_same_size_raw_drift_and_extra_evidence() {
    let workspace = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&workspace).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("restore-database-images-")
        .tempdir_in(&workspace)
        .unwrap();
    let backend = portable_path(&temp.path().canonicalize().unwrap());
    let directory = backend.join(".local-tests/phase/inventory");
    fs::create_dir_all(&directory).unwrap();
    for phase in ["before", "after"] {
        for (key, _, _) in TARGET_DATABASES {
            fs::write(
                directory.join(format!("{phase}-target-{key}.json")),
                b"original\n",
            )
            .unwrap();
        }
    }
    let files = tree::snapshot(&backend, &directory, false).unwrap();
    assert_eq!(files.as_array().unwrap().len(), 8);
    check_tree(&backend, &directory, &files).unwrap();
    let raw = directory.join("after-target-shared.json");
    fs::write(&raw, b"modified\n").unwrap();
    assert!(check_tree(&backend, &directory, &files).is_err());
    fs::write(raw, b"original\n").unwrap();
    fs::write(directory.join("failure.json"), b"retained failure").unwrap();
    assert!(check_tree(&backend, &directory, &files).is_err());
}
