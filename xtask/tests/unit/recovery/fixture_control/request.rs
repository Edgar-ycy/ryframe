use std::fs;

use super::*;

#[test]
fn fresh_request_keeps_legacy_projection_and_rejects_different_rustfs_tool() {
    let backend = portable_path(&crate::workspace::root_dir().canonicalize().unwrap());
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    let directory = portable_path(&directory.path().canonicalize().unwrap());
    let credentials = directory.join("mysql-client.cnf");
    fs::write(&credentials, b"fixture credential").unwrap();
    let selected = json!({
        "scope_id":"fixture-base-demo",
        "objects":{"endpoint":"http://127.0.0.1:9010","region":"local"},
        "databases":request::KEYS.iter().rev().map(|(key, _, _)| json!({
            "key":key,"database":format!("demo-{key}"),"expected_server_uuid":"server-id",
            "connection_file":credentials,
        })).collect::<Vec<_>>(),
    });
    let defaults_sha256 = digest::file(&credentials).unwrap().sha256;
    let databases = database_bindings(&backend, &selected).unwrap();
    let expected_databases = request::KEYS
        .iter()
        .map(|(key, kind, mode)| json!({
            "key":key,"kind":kind,"mode":mode,"database":format!("demo-{key}"),
            "server_uuid":"server-id","defaults_file":credentials,"defaults_sha256":defaults_sha256,
        }))
        .collect::<Vec<_>>();
    let review =
        json!({"path":"ready.json","bytes":1,"sha256":"review","canonical_sha256":"canonical"});
    let maintenance = json!({"path":"build.json","bytes":2,"sha256":"maintenance"});
    let publication = json!({"path":"execution-source.json","bytes":3,"sha256":"publication"});
    let mysql = json!({"path":"mysql.exe","sha256":"mysql"});
    let rustfs = json!({"identity":{"pid":123,"started":"42","executable":"rustfs.exe"},
        "sha256":"rustfs","process_receipt":{"path":"process.json"},"launch_receipt":{"path":"launch.json"}});
    let redis = json!({"platform":"windows","pid":456,"started":"43","run_id":"same-generation"});
    let storage = json!({"rustfs":rustfs,"redis":redis});
    let execution = directory.join("backend");
    let configuration = execution.join("environment/config");
    let product = Product {
        prepared: Prepared {
            path: directory.join("bootstrap.json"),
            binding: Value::Null,
            receipt: json!({"plan":{"review":review,"maintenance_build":maintenance,"execution_source":publication}}),
            review_path: directory.join("ready.json"),
            review: json!({"scopes":{"base":selected},"tools":{"mysql":mysql,
                "rustfs":{"path":"rustfs.exe","sha256":"rustfs"}}}),
        },
        execution: execution.clone(),
        environment: directory.join("environment.json"),
        environment_binding: Value::Null,
        configuration: configuration.clone(),
        configuration_sha256: "config-sha".into(),
        databases,
        settings: json!({"object_storage":{"access_key":"fixture-access","secret_key":"fixture-secret"},
            "reset":{"credential_version":"fixture-v1"}}),
    };
    let expected = json!({
        "format_version":1,"kind":"devex-clone-fresh-target","id":"request-base-demo",
        "review":review,"side":"base",
        "target":{"scope_id":"fixture-base-demo","s3":{
            "endpoint":"http://127.0.0.1:9010","region":"local","access_key":"fixture-access","secret_key":"fixture-secret"},
            "databases":expected_databases},
        "maintenance_build":maintenance,"configuration_sha256":"config-sha","config_dir":configuration,
        "tools":{"mysql":mysql},"storage":storage,
        "reset":{"legacy_ownership":{"mysql_exclusive":true,"redis_exclusive":true,"object_storage_exclusive":true},
            "credential_version":"fixture-v1","sentinel_key":"ryframe:devex-fresh:fixture-base-demo:sentinel",
            "sentinel_value":"devex-fresh:request-base-demo"},
        "execution_backend":{"execution_source":publication,"path":execution},
    });
    assert_eq!(
        product
            .request("request-base-demo", "base", &storage)
            .unwrap(),
        expected
    );
    let mut drifted = storage;
    drifted["rustfs"]["sha256"] = json!("different");
    assert!(
        product
            .request("request-base-demo", "base", &drifted)
            .is_err()
    );
}

#[test]
fn publish_refuses_input_drift_and_preserves_the_first_output() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("request.json");
    let original = json!({"storage":{"generation":"first"}});
    let changed = json!({"storage":{"generation":"second"}});
    let mut snapshots = [original.clone(), changed.clone()].into_iter();
    assert!(publish(&output, || Ok(snapshots.next().unwrap())).is_err());
    assert_eq!(document::read(&output).unwrap(), original);
    assert!(publish(&output, || Ok(changed.clone())).is_err());
    assert_eq!(document::read(&output).unwrap(), original);

    let next = directory.path().join("accepted.json");
    assert_eq!(publish(&next, || Ok(changed.clone())).unwrap(), changed);
    assert_eq!(document::read(&next).unwrap(), changed);
}

#[test]
fn product_guard_rechecks_actual_selected_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let directory = portable_path(&directory.path().canonicalize().unwrap());
    let bootstrap = directory.join("bootstrap.json");
    let review = directory.join("review.json");
    let environment = directory.join("environment.json");
    for path in [&bootstrap, &review, &environment] {
        fs::write(path, b"{}").unwrap();
    }
    let configuration = directory.join("config");
    fs::create_dir(&configuration).unwrap();
    let selected = configuration.join("app.test.toml");
    fs::write(&selected, b"scope_id = \"first\"\n").unwrap();
    let product = Product {
        prepared: Prepared {
            binding: file_binding(&bootstrap, "fixture").unwrap(),
            path: bootstrap,
            receipt: json!({"plan":{"review":file_binding(&review, "fixture").unwrap()}}),
            review_path: review,
            review: Value::Null,
        },
        execution: directory,
        environment_binding: file_binding(&environment, "fixture").unwrap(),
        environment,
        configuration_sha256: configuration_digest(&configuration).unwrap(),
        configuration,
        databases: vec![],
        settings: Value::Null,
    };
    product.check().unwrap();
    fs::write(selected, b"scope_id = \"second\"\n").unwrap();
    assert!(product.check().is_err());
}
