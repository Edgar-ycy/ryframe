use super::*;

fn input() -> (Value, BTreeMap<String, Value>) {
    let identity = |pid, started| {
        json!({"pid":pid,"started":started,
        "executable":crate::workspace::root_dir().join("fixture.exe")})
    };
    let api = identity(20, "123");
    let worker = identity(21, "124");
    let controller = identity(20, "125");
    let registry = json!({"format_version":1,"scope_id":"source-scope","runtime_sha256":"a".repeat(64),
        "processes":[{"name":"worker","identity":worker},
        {"name":"history-0001","identity":controller},{"name":"api","identity":api}]});
    (
        registry,
        BTreeMap::from([("api".into(), api), ("worker".into(), worker)]),
    )
}

#[test]
fn complete_registry_keeps_distinct_pid_generations_and_sorts_all_names() {
    let (registry, processes) = input();
    let value = validate_registry(
        &registry,
        &json!("source-scope"),
        &"a".repeat(64),
        &processes,
    )
    .unwrap();
    assert_eq!(
        value
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["api", "history-0001", "worker"]
    );
    assert_eq!(value[1]["identity"]["pid"], value[0]["identity"]["pid"]);
    assert_ne!(
        value[1]["identity"]["started"],
        value[0]["identity"]["started"]
    );
}

#[test]
fn different_names_cannot_duplicate_creation_identity_or_replace_runtime_roles() {
    let (registry, processes) = input();
    for change in [
        "duplicate-identity",
        "duplicate-name",
        "missing-api",
        "wrong-api",
        "wrong-scope",
        "wrong-runtime",
    ] {
        let mut modified = registry.clone();
        match change {
            "duplicate-identity" => modified["processes"][1]["identity"] = processes["api"].clone(),
            "duplicate-name" => modified["processes"][1]["name"] = json!("api"),
            "missing-api" => modified["processes"][2]["name"] = json!("unrelated"),
            "wrong-api" => modified["processes"][2]["identity"]["started"] = json!("126"),
            "wrong-scope" => modified["scope_id"] = json!("other-scope"),
            _ => modified["runtime_sha256"] = json!("b".repeat(64)),
        }
        assert!(
            validate_registry(
                &modified,
                &json!("source-scope"),
                &"a".repeat(64),
                &processes
            )
            .is_err(),
            "{change}"
        );
    }
}

#[test]
fn runtime_receipts_bind_both_roles_scope_and_the_verified_artifact_executable() {
    let backend = crate::workspace::root_dir();
    std::fs::create_dir_all(backend.join(".local-tests")).unwrap();
    let holder = tempfile::Builder::new()
        .prefix("export-runtime-proof-")
        .tempdir_in(backend.join(".local-tests"))
        .unwrap();
    let mut request = json!({"source":{"scope_id":"source-scope"},"processes":{}});
    let mut artifacts = serde_json::Map::new();
    let mut runtime = json!({"artifacts":{}});
    for (index, role) in ["api", "worker"].iter().enumerate() {
        let executable = holder.path().join(format!("{role}.exe"));
        let identity = json!({"pid":20+index,"started":"123","executable":executable});
        let receipt =
            json!({"format_version":1,"role":role,"scope_id":"source-scope","identity":identity});
        let path = holder.path().join(format!("{role}.json"));
        json_io::write_new(&path, &receipt).unwrap();
        request["processes"][role] = common::binding(&path).unwrap();
        artifacts.insert(
            (*role).into(),
            json!({"executable":executable,
            "command":["verified-by-build-stage"],"bytes":1,"sha256":"a".repeat(64)}),
        );
        runtime["artifacts"][role] = json!({"path":executable,"sha256":"a".repeat(64)});
    }
    let mut context = Context {
        backend: backend.clone(),
        run_dir: holder.path().to_owned(),
        mode: "run".into(),
        attempt: 1,
        manifest: Value::Null,
        request,
        object_identity: json!({}),
        request_path: holder.path().join("request.json"),
        environment: BTreeMap::new(),
        execution_backend: backend,
    };
    let build = json!({"artifacts":artifacts});
    assert_eq!(
        runtime_processes(&context, &build, &runtime).unwrap().len(),
        2
    );
    let path = holder.path().join("api.json");
    let original = json_io::read(&path).unwrap();
    for (pointer, value) in [
        ("/role", json!("worker")),
        ("/scope_id", json!("other-scope")),
        (
            "/identity/executable",
            json!(holder.path().join("other.exe")),
        ),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        context.request["processes"]["api"] = common::binding(&path).unwrap();
        assert!(
            runtime_processes(&context, &build, &runtime).is_err(),
            "{pointer}"
        );
    }
}

fn full_runtime_fixture(root: &Path) -> (Context, Value) {
    let directory = root.join("runtime");
    std::fs::create_dir(&directory).unwrap();
    for (name, id) in [("config", 3), ("worker-config", 2)] {
        let path = root.join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("app.test.toml"), format!(
            "scope_id='runtime-test'\nsnowflake_worker_id={id}\n[jobs]\nmode='external'\nhealth_host='127.0.0.1'\nhealth_port=18081\n"
        )).unwrap();
    }
    let mut binaries = serde_json::Map::new();
    let mut artifacts = serde_json::Map::new();
    for (name, role) in [
        ("ryframe", "api"),
        ("ryframe-worker", "worker"),
        ("ryframe-reset", "reset"),
        ("ryframe-migrate", "migrate"),
    ] {
        let path = directory.join(format!("{name}.exe"));
        std::fs::write(&path, name).unwrap();
        binaries.insert(name.into(), json!(path));
        if matches!(role, "api" | "worker") {
            artifacts.insert(
                role.into(),
                json!({"path":path,"sha256":digest::file(&path).unwrap().sha256}),
            );
        }
    }
    json_io::write_new(&directory.join("binaries.json"), &json!(binaries)).unwrap();
    let runtime = json!({"format_version":1,"backend_root":root,"scope_id":"runtime-test",
        "api_config_dir":root.join("config"),"worker_config_dir":root.join("worker-config"),
        "api_configuration_sha256":"","worker_configuration_sha256":"","configuration_sha256":"",
        "worker_ready_url":"http://127.0.0.1:18081/readyz","artifacts":artifacts});
    let context = Context {
        backend: crate::workspace::root_dir(),
        run_dir: root.to_owned(),
        mode: "source-register".into(),
        attempt: 1,
        manifest: Value::Null,
        object_identity: json!({}),
        request: json!({}),
        request_path: root.join("request.json"),
        environment: BTreeMap::from([("APP_ENV".into(), "test".into())]),
        execution_backend: root.to_owned(),
    };
    (context, runtime)
}

fn publish_runtime(context: &mut Context, runtime: &mut Value) -> std::path::PathBuf {
    use crate::recovery::clone::native::target_verify::input::request_configuration::python_configuration_hash;
    let mut hashes = Vec::new();
    for role in ["api", "worker"] {
        let directory =
            std::path::PathBuf::from(runtime[format!("{role}_config_dir")].as_str().unwrap());
        let sha = digest::file(&directory.join("app.test.toml"))
            .unwrap()
            .sha256;
        let hash = python_configuration_hash(&json!({"environment":{"APP_ENV":"test"},
            "config_directory":directory,"files":{"app.test.toml":sha}}))
        .unwrap();
        runtime[format!("{role}_configuration_sha256")] = json!(hash);
        hashes.push(hash);
    }
    runtime["configuration_sha256"] = json!(python_configuration_hash(&json!(hashes)).unwrap());
    let path = context.run_dir.join("runtime/runtime.json");
    std::fs::write(&path, serde_json::to_vec(runtime).unwrap()).unwrap();
    context.request["runtime"] = common::binding(&path).unwrap();
    path
}

#[test]
fn full_runtime_contract_rejects_configuration_mismatch_duplicate_instances_and_extra_fields() {
    let local = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let holder = tempfile::Builder::new()
        .prefix("export-full-runtime-")
        .tempdir_in(&local)
        .unwrap();
    let (mut context, mut runtime) = full_runtime_fixture(holder.path());
    let path = publish_runtime(&mut context, &mut runtime);
    verify_runtime_contract(&context, &path, &runtime).unwrap();
    let worker = holder.path().join("worker-config/app.test.toml");
    let original = std::fs::read_to_string(&worker).unwrap();
    for text in [
        original.replace("health_port=18081", "health_port=18082"),
        original.replace("snowflake_worker_id=2", "snowflake_worker_id=3"),
    ] {
        std::fs::write(&worker, text).unwrap();
        publish_runtime(&mut context, &mut runtime);
        assert!(verify_runtime_contract(&context, &path, &runtime).is_err());
    }
    std::fs::write(&worker, original).unwrap();
    runtime["unexpected"] = json!(true);
    publish_runtime(&mut context, &mut runtime);
    assert!(verify_runtime_contract(&context, &path, &runtime).is_err());
    runtime.as_object_mut().unwrap().remove("unexpected");
    publish_runtime(&mut context, &mut runtime);
    let mut old = runtime.clone();
    old["scope_id"] = json!("other-scope");
    assert!(verify_runtime_contract(&context, &path, &old).is_err());
    context
        .environment
        .insert("UNEXPECTED".into(), "private".into());
    assert!(verify_runtime_contract(&context, &path, &runtime).is_err());
}
