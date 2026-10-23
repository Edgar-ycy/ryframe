use super::*;

fn request(operation: &str, arguments: Value) -> (i32, Value) {
    execute(
        &serde_json::to_vec(
            &json!({"format_version":1,"operation":operation,"arguments":arguments,"write":false}),
        )
        .unwrap(),
    )
}

#[test]
fn passwords_are_ascii_bounded_and_never_echoed() {
    for password in ["Aa1!xxxx", &format!("Aa1!{}", "x".repeat(68))] {
        assert_eq!(
            request("reset-password", json!({"admin_password":password})).0,
            0
        );
    }
    for password in [
        json!(null),
        json!(""),
        json!("123456"),
        json!("Onlyletters"),
        json!("Aa1!中文xx"),
        json!("Aa1!xxx\n"),
        json!(format!("Aa1!{}", "x".repeat(69))),
    ] {
        let (code, value) = request("reset-password", json!({"admin_password":password}));
        assert_ne!(code, 0);
        assert_eq!(
            value["error"]["message"],
            "admin_password 不满足 reset 管理员密码复杂度策略"
        );
        assert_eq!(value["error"]["remote_write_may_have_occurred"], false);
    }
}

#[test]
fn protocol_rejects_duplicates_unknown_fields_writes_and_truncation() {
    for raw in [
        br#"{}"#.as_slice(),
        br#"{"format_version":1,"format_version":1,"operation":"fixture","arguments":{},"write":false}"#,
        br#"{"format_version":true,"operation":"fixture","arguments":{},"write":false}"#,
        br#"{"format_version":1,"operation":"fixture","arguments":{},"write":true}"#,
        br#"{"format_version":1,"operation":"fixture","arguments":{},"write":false,"extra":1}"#,
        br#"{"format_version":1,"operation":"fixture","arguments":{},"write":false"#,
    ] { assert_ne!(execute(raw).0, 0); }
    assert_ne!(execute(&vec![b'x'; LIMIT + 1]).0, 0);
}

#[test]
fn fixture_and_maintenance_types_fail_closed() {
    let fixture = json!({"format_version":1,"fixture":"business","status":"ready",
        "sources":{"backend":{"head":"abc"}},"paths":{"backend":"b","frontend":"f"}});
    assert_eq!(request("fixture", fixture.clone()).0, 0);
    for (field, value) in [
        ("format_version", json!(true)),
        ("status", json!("pending")),
        ("sources", json!({"backend":{}})),
        ("paths", json!({"backend":"b"})),
    ] {
        let mut invalid = fixture.clone();
        invalid[field] = value;
        assert_ne!(request("fixture", invalid).0, 0);
    }
    let mut build = json!({"format_version":1,"kind":"devex-clone-tool-build","resources_modified":false,
        "artifacts":{"reset":{"executable":"reset"},"migrate":{"executable":"migrate"},"tenant-data":{"executable":"tenant"}}});
    assert_eq!(request("maintenance", build.clone()).0, 0);
    build["resources_modified"] = json!(0);
    assert_ne!(request("maintenance", build).0, 0);
}

fn review(root: &std::path::Path) -> Value {
    let execution = root.join("business-backend");
    let mut value = json!({"kind":"review-only-perf-resource-plan-with-readonly-preflight","ready_for_execution":true,
        "reference":{"source":{"databases":[]},"protected_target":{"databases":[]}},
        "tools":{"mysql":{"path":"mysql","sha256":"a"},"mysqldump":{"path":"dump","sha256":"b"},
            "rustfs":{"path":"rustfs","sha256":"c"},"redis_server":{"distribution":"linux","resolved_path":"redis","sha256":"d"},
            "wsl":{"path":"frozen-launcher","sha256":"e"},"redis_python":{"path":"frozen-python","sha256":"f"},
            "historical":{"path":"retained","sha256":"history"}},
        "future_root":execution.join(".local-tests/reference-fixture/run-r1"),
        "services":{"rustfs":{"api":"http://127.0.0.1:29200","console":"http://127.0.0.1:29201","data_dir":"data"},"redis":{"directory":"redis"}},"scopes":{}});
    for side in ["seed", "base", "candidate"] {
        value["scopes"][side] = json!({"scope_id":format!("fixture-{side}"),"backend_dir":execution,
            "runtime_dir":"runtime","identity_ledger":"identity","api_url":"http://127.0.0.1:18210",
            "worker_ready_url":"http://127.0.0.1:19210/readyz","frontend_url":"http://127.0.0.1:4190",
            "objects":{"endpoint":"https://objects.example:9000","region":"us-east-1"},
            "redis":{"url":"redis://127.0.0.1:16390/0","namespace":"namespace","ownership_key":"key","ownership_value":"owner"},
            "databases":(["shared-control","shared","dedicated-a","dedicated-b"].iter().map(|key|
                json!({"key":key,"database":format!("fixture_{}_{}",side,key.replace('-', "_")),
                    "expected_server_uuid":"uuid","host":"127.0.0.1","port":3306,"mode":if key.starts_with("dedicated") {"dedicated"} else {"shared"}})).collect::<Vec<_>>())});
    }
    value
}
fn ready(mut value: Value) -> Value {
    let run = std::path::Path::new(value["future_root"].as_str().unwrap()).join("service-run");
    value["ready_for_execution"] = json!(true);
    value["services"]["rustfs"]["scope_id"] = json!("services-fixture-seed");
    value["services"]["rustfs"]["process_receipt"] = json!(run.join("rustfs").join("process.json"));
    let tools = super::super::guards::TOOLS
        .iter()
        .map(|key| ((*key).to_owned(), value["tools"][*key].clone()))
        .collect::<serde_json::Map<_, _>>();
    value["preflight"] = json!({"format_version":1,"kind":"reference-fixture-tool-preflight","status":"verified",
        "supersedes":{"path":"previous","bytes":1,"sha256":"digest"},"tools":tools});
    value
}

#[test]
fn successor_allows_only_reviewed_tool_refresh_and_rejects_historical_or_resource_drift() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut pending = review(&root);
    pending["ready_for_execution"] = json!(false);
    let current = ready(pending.clone());
    assert_eq!(request("preflight-binding", current.clone()).0, 0);
    assert_eq!(
        request(
            "preflight-successor",
            json!({"previous":pending,"review":current})
        )
        .0,
        0
    );
    for field in ["scope_id", "process_receipt"] {
        let mut invalid = current.clone();
        invalid["services"]["rustfs"][field] = json!("drift");
        assert_ne!(request("preflight-binding", invalid).0, 0);
    }
    let mut extra = current.clone();
    extra["preflight"]["unexpected"] = json!(true);
    assert_ne!(request("preflight-binding", extra).0, 0);
    for field in ["path", "sha256"] {
        let mut invalid = current.clone();
        invalid["tools"]["historical"][field] = json!("drift");
        assert_ne!(
            request(
                "preflight-successor",
                json!({"previous":pending,"review":invalid})
            )
            .0,
            0
        );
    }
    let mut updated = current.clone();
    updated["tools"]["mysql"]["sha256"] = json!("changed");
    updated["preflight"]["tools"]["mysql"] = updated["tools"]["mysql"].clone();
    assert_eq!(
        request(
            "preflight-successor",
            json!({"previous":current,"review":updated})
        )
        .0,
        0
    );
}

#[test]
fn configuration_roundtrips_unicode_credentials_ipv6_and_explicit_default_ports() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut selected = review(&root)["scopes"]["seed"].clone();
    selected["api_url"] = json!("http://[::1]:80");
    selected["worker_ready_url"] = json!("https://[::1]:443/readyz");
    selected["frontend_url"] = json!("http://[::1]:80/");
    let client = json!({"tls_mode":"verify-identity","host":"db.example","port":"3306"});
    let values = json!({"admin_password":"Admin1!fixture","access_key":"中文 access","secret_key":"quote\"slash\\line\n",
        "redis_password":"redis","jwt_secret":"jwt","metrics_token":"metric"});
    let (code, response) = request(
        "configuration-settings",
        json!({"selected":selected,"client":client,"values":values,"side":"base"}),
    );
    assert_eq!(code, 0, "{response}");
    let settings = &response["result"]["settings"];
    assert_eq!(settings["app"], json!({"host":"::1","port":80}));
    assert_eq!(settings["jobs"]["health_port"], 443);
    assert_eq!(settings["jobs"]["scheduler_enabled"], false);
    assert_eq!(
        settings["cors"]["allow_origins"],
        json!(["http://[::1]:80"])
    );
    let (code, text) = request("configuration-text", response["result"].clone());
    assert_eq!(code, 0, "{text}");
    let parsed: toml::Table = toml::from_str(text["result"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        parsed["object_storage"]["secret_key"].as_str(),
        values["secret_key"].as_str()
    );
    assert_eq!(
        parsed["database"]["primary"]["tls_mode"].as_str(),
        Some("verify-identity")
    );
    assert_eq!(
        parsed["tenant_data"]["targets"].as_array().unwrap().len(),
        3
    );
    selected["frontend_url"] = json!("http://127.0.0.1:80/other");
    assert_ne!(
        request(
            "configuration-settings",
            json!({"selected":selected,"client":client,"values":values,"side":"seed"})
        )
        .0,
        0
    );
}

#[test]
fn plan_is_readonly_hashes_exact_inputs_and_rejects_every_input_drift() {
    use crate::recovery::clone::native::{digest, json as native_json};
    let backend = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let parent = backend.join(".local-tests/reference-environment-native");
    std::fs::create_dir_all(&parent).unwrap();
    let temporary = tempfile::tempdir_in(&parent).unwrap();
    let root = super::super::super::portable_path(temporary.path());
    let review = ready(review(&root));
    let execution = root.join("fixture/backend");
    let reference = execution.join(".local-tests/reference-fixture");
    std::fs::create_dir_all(&reference).unwrap();
    let fixture = json!({"format_version":1,"fixture":"business","status":"ready","sources":{"backend":{"head":"abc"}},
        "paths":{"backend":execution,"frontend":root.join("fixture/frontend")}});
    let maintenance = json!({"format_version":1,"kind":"devex-clone-tool-build","resources_modified":false,
        "artifacts":{"reset":{"executable":"reset"},"migrate":{"executable":"migrate"},"tenant-data":{"executable":"tenant"}}});
    let documents = [
        ("review", review),
        ("fixture", fixture),
        ("maintenance_build", maintenance),
    ];
    for (name, value) in &documents {
        native_json::write_new(&root.join(format!("{name}.json")), value).unwrap();
    }
    let args = json!({"backend":backend,"review":root.join("review.json").strip_prefix(&backend).unwrap(),
        "fixture":root.join("fixture.json"),"maintenance_build":root.join("maintenance_build.json"),"side":"seed"});
    let (code, response) = request("plan", args.clone());
    assert_eq!(code, 0, "{response}");
    let plan = response["result"].clone();
    let mut unsigned = plan.clone();
    unsigned.as_object_mut().unwrap().remove("sha256");
    assert_eq!(plan["sha256"], digest::plan_hash(&unsigned).unwrap());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 4);
    let output = reference.join("environment-seed-r1");
    let (code, allowed) = request(
        "prepare-output",
        json!({"backend":backend,"plan":plan,"output":output}),
    );
    assert_eq!(code, 0, "{allowed}");
    assert!(!output.exists());
    let wrong = root.join("coordinator-only-environment");
    let (code, denied) = request(
        "prepare-output",
        json!({"backend":backend,"plan":plan,"output":wrong}),
    );
    assert_ne!(code, 0);
    assert!(
        denied["error"]["message"]
            .as_str()
            .unwrap()
            .contains(reference.to_str().unwrap())
    );
    assert!(!wrong.exists());
    assert!(std::fs::read_dir(&reference).unwrap().next().is_none());
    assert_eq!(
        request("planned-inputs", json!({"backend":backend,"plan":plan})).0,
        0
    );
    for (name, _value) in &documents {
        let path = root.join(format!("{name}.json"));
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"{}").unwrap();
        assert_ne!(
            request("planned-inputs", json!({"backend":backend,"plan":plan})).0,
            0
        );
        std::fs::write(path, original).unwrap();
    }
    let mut historical = documents[0].1.clone();
    historical["reference"]["source"]["databases"] = json!([{"database":"old"}]);
    std::fs::write(
        root.join("review.json"),
        serde_json::to_vec(&historical).unwrap(),
    )
    .unwrap();
    assert_ne!(request("plan", args).0, 0);
}

#[test]
fn administrator_credentials_constructs_distinct_strong_private_toml_and_rejects_extra_fields() {
    let initial = "Initial!Only123";
    let (code, value) = request(
        "administrator-credentials",
        json!({"admin_password":initial}),
    );
    assert_eq!(code, 0);
    let table: toml::Table = toml::from_str(value["result"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(table.len(), 1);
    let account = table["administrator"].as_table().unwrap();
    assert_eq!(account.len(), 3);
    assert_eq!(account["tenant_id"].as_str(), Some("system"));
    assert_eq!(account["username"].as_str(), Some("admin"));
    let password = account["password"].as_str().unwrap();
    assert_ne!(password, initial);
    assert_eq!(
        request("reset-password", json!({"admin_password":password})).0,
        0
    );
    let (code, value) = request(
        "administrator-credentials",
        json!({"admin_password":initial,"password":"private-sentinel"}),
    );
    assert_ne!(code, 0);
    assert!(!value.to_string().contains(initial));
    assert!(!value.to_string().contains("private-sentinel"));
}

#[test]
fn environment_output_belongs_to_both_coordinator_and_execution_without_writes() {
    let coordinator = crate::workspace::root_dir();
    let parent = coordinator.join(".local-tests/environment-output-native");
    std::fs::create_dir_all(&parent).unwrap();
    let temporary = tempfile::tempdir_in(&parent).unwrap();
    let execution = temporary.path().join("fixture/backend");
    let reference = execution.join(".local-tests/reference-fixture");
    std::fs::create_dir_all(&reference).unwrap();
    let fixture = json!({"format_version":1,"fixture":"business","status":"ready",
        "sources":{"backend":{"head":"abc"}},"paths":{"backend":execution,"frontend":temporary.path().join("fixture/frontend")}});
    let valid = reference.join("environment-seed-r1");
    assert_ne!(coordinator, execution);
    let before = std::fs::read_dir(temporary.path()).unwrap().count();
    assert_eq!(
        super::super::output::validate(&coordinator, &fixture, &valid).unwrap(),
        valid
    );
    assert!(!valid.exists());
    let wrong = temporary.path().join("coordinator-only");
    let error = super::super::output::validate(&coordinator, &fixture, &wrong)
        .unwrap_err()
        .to_string();
    assert!(error.contains(reference.to_str().unwrap()));
    assert!(!wrong.exists());
    assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), before);
    assert!(std::fs::read_dir(&reference).unwrap().next().is_none());
    std::fs::create_dir(&valid).unwrap();
    assert!(super::super::output::validate(&coordinator, &fixture, &valid).is_err());
}
