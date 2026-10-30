use super::*;
use crate::recovery::{
    clone::native::{digest, s3::app_configuration},
    inputs::domain,
};

struct Fixture {
    _temporary: tempfile::TempDir,
    backend: std::path::PathBuf,
    output: std::path::PathBuf,
    bootstrap_file: std::path::PathBuf,
    review_file: std::path::PathBuf,
    configuration: Value,
    review: Value,
    bootstrap: Value,
}
impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let local = backend.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temporary = tempfile::tempdir_in(&local).unwrap();
        let output = plan::canonical(temporary.path()).unwrap();
        let defaults = output.join("mysql.cnf");
        fs::write(
            &defaults,
            "[client]\nuser=fixture\npassword=Private!Database123\n",
        )
        .unwrap();
        let administrator = output.join("administrator.toml");
        fs::write(
        &administrator,
        "[administrator]\ntenant_id='system'\nusername='admin'\npassword='Current!Changed123'\n",
    )
    .unwrap();
        let tool = output.join("tool");
        fs::write(&tool, b"fixture-not-executable").unwrap();
        let config = output.join("config");
        fs::create_dir(&config).unwrap();
        fs::write(
            config.join("app.test.toml"),
            concat!(
                "[object_storage]\nendpoint='http://127.0.0.1:23990'\nregion='us-east-1'\n",
                "access_key='PrivateAccess'\nsecret_key='PrivateSecret'\n",
            ),
        )
        .unwrap();
        let configuration =
            app_configuration::configuration_value(&backend, &config, "test").unwrap();
        let bootstrap_file = output.join("bootstrap.json");
        let review_file = output.join("review.json");
        for file in [
            &bootstrap_file,
            &review_file,
            &output.join("runtime.json"),
            &output.join("source-pair.json"),
        ] {
            write_new(file, &json!({"fixture":true})).unwrap();
        }
        let scope = |side: &str, offset: u16| {
            json!({"scope_id":format!("fixture-{side}-r1"),
        "runtime_dir":output.join(format!("runtime-{side}")),"api_url":format!("http://127.0.0.1:{}",23000+offset),
        "worker_ready_url":format!("http://127.0.0.1:{}/readyz",23010+offset),
        "frontend_url":format!("http://127.0.0.1:{}",23020+offset),
        "objects":{"endpoint":"http://127.0.0.1:23990","region":"us-east-1"},
        "databases":domain::TARGET_DATABASES.iter().map(|(key,_,_)|json!({"key":key,"database":format!("{side}_{}",key.replace('-',"_")),
            "expected_server_uuid":"12345678-1234-1234-1234-123456789abc","connection_file":defaults})).collect::<Vec<_>>()})
        };
        let review = json!({"scopes":{"seed":scope("seed",0),"base":scope("base",1),"candidate":scope("candidate",2)},
        "tools":{"mysql":{"path":tool,"sha256":digest::file(&tool).unwrap().sha256},"mysqldump":{"path":tool,"sha256":digest::file(&tool).unwrap().sha256}},
        "dataset":{"records":100000,"object_count":256,"object_bytes":4194304,"request_interval_ms":1000,
            "tenant_targets":["shared","shared","shared","shared","shared","shared","shared","shared","shared","dedicated-a"]}});
        let bootstrap = json!({"administrator_credentials":bound(&administrator).unwrap()});
        Self {
            _temporary: temporary,
            backend,
            output,
            bootstrap_file,
            review_file,
            configuration,
            review,
            bootstrap,
        }
    }
}

#[test]
fn reviewed_dataset_plan_keeps_both_sides_scale_current_credentials_and_binding_bytes() {
    let Fixture {
        _temporary,
        backend,
        output,
        bootstrap_file,
        review_file,
        configuration,
        mut review,
        bootstrap,
    } = Fixture::new();
    for side in ["base", "candidate"] {
        let result = plan::assemble(
            (&output, &json!("fixture-seed-r1")),
            &output.join("work"),
            side,
            &review,
            &configuration,
            (&bootstrap, &bootstrap_file, &review_file),
        )
        .unwrap();
        domain::validate_plan(&result, &backend).unwrap();
        assert_eq!(result["source"]["runtime_dir"], json!(output));
        assert_eq!(result["target"]["scope_id"], format!("fixture-{side}-r1"));
        assert_eq!(result["target_side"], side);
        for (field, expected) in review["dataset"].as_object().unwrap() {
            assert_eq!(&result["dataset"][field], expected);
        }
        assert_eq!(result["dataset"]["timeout_seconds"], 43200);
        let bytes = serde_json::to_string(&result).unwrap();
        for secret in [
            "Current!Changed123",
            "PrivateAccess",
            "PrivateSecret",
            "Private!Database123",
        ] {
            assert!(!bytes.contains(secret));
        }
        let mut collision = result.clone();
        collision["target"]["worker_ready_url"] = json!(format!(
            "{}/readyz",
            text(&collision["target"]["api_url"]).unwrap()
        ));
        assert!(domain::validate_plan(&collision, &backend).is_err());
    }
    assert!(
        plan::assemble(
            (&output, &json!("other-scope")),
            &output.join("work"),
            "base",
            &review,
            &configuration,
            (&bootstrap, &bootstrap_file, &review_file)
        )
        .is_err()
    );
    let mut invalid = review["scopes"]["base"].clone();
    let duplicate = invalid["databases"][0].clone();
    invalid["databases"].as_array_mut().unwrap().push(duplicate);
    assert!(plan::scope(&invalid, &configuration).is_err());
    let mut inline = configuration.clone();
    inline["secret_key"] = json!("PrivateSecret");
    assert!(plan::scope(&review["scopes"]["base"], &inline).is_err());
    review["dataset"].as_object_mut().unwrap().remove("records");
    assert!(
        plan::assemble(
            (&output, &json!("fixture-seed-r1")),
            &output.join("work"),
            "base",
            &review,
            &configuration,
            (&bootstrap, &bootstrap_file, &review_file)
        )
        .is_err()
    );
}

#[test]
fn unknown_write_diagnostic_keeps_exit_redacts_and_never_overwrites_original_evidence() {
    #[derive(Debug)]
    struct Changed(std::io::Error);
    impl std::fmt::Display for Changed {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("配置或来源PrivateSecret变化")
        }
    }
    impl std::error::Error for Changed {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }
    let fixture = Fixture::new();
    let root = &fixture.output;
    let secrets = vec![
        "Private!Current123".into(),
        "PrivateAccess".into(),
        "PrivateSecret".into(),
    ];
    let stdout = execute::redact(b"Private!Current123 PrivateAccess", &secrets);
    let stderr = execute::redact(b"PrivateSecret", &secrets);
    let error = Changed(std::io::Error::from_raw_os_error(5));
    for (phase, attempted, code) in [
        ("input_guard", false, None),
        ("post_guard", true, Some(0)),
        ("report_publication", true, Some(7)),
    ] {
        let file = root.join(format!("{phase}.prepare-failed.json"));
        let record =
            execute::failure_record(phase, attempted, code, &error, &stdout, &stderr, &secrets);
        execute::publish_failure(&file, root, &record).unwrap();
        let before = fs::read(&file).unwrap();
        let value = document(&file).unwrap().0;
        assert_eq!(value["returncode"], json!(code));
        assert_eq!(value["os_code"], 5);
        assert_eq!(value["stage"], phase);
        assert_eq!(value["launch_attempted"], attempted);
        assert_eq!(
            value["write_outcome"],
            if attempted { "unknown" } else { "not_started" }
        );
        assert_eq!(
            value["status"],
            if attempted {
                "failed_unknown_writes"
            } else {
                "failed_not_started"
            }
        );
        assert!(value["cause"].as_str().unwrap().contains("[REDACTED]"));
        for secret in &secrets {
            assert!(!String::from_utf8_lossy(&before).contains(secret.as_str()));
        }
        assert!(execute::publish_failure(&file, root, &json!({"other":true})).is_err());
        assert_eq!(fs::read(&file).unwrap(), before);
    }
}
