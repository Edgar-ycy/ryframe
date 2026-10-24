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
