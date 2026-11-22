use super::*;

fn table_text(value: &Value) -> String {
    fn convert(value: &Value) -> toml::Value {
        match value {
            Value::String(value) => toml::Value::String(value.clone()),
            Value::Bool(value) => toml::Value::Boolean(*value),
            Value::Number(value) => toml::Value::Integer(value.as_i64().unwrap()),
            Value::Array(values) => toml::Value::Array(values.iter().map(convert).collect()),
            Value::Object(values) => toml::Value::Table(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), convert(value)))
                    .collect(),
            ),
            Value::Null => panic!("测试配置不包含null"),
        }
    }
    toml::to_string(&convert(value)).unwrap()
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    table: Value,
    source: Value,
}
impl Fixture {
    fn new() -> Self {
        let base = crate::workspace::root_dir().join(".local-tests/source-binding-unit");
        fs::create_dir_all(&base).unwrap();
        let temp = tempfile::tempdir_in(base).unwrap();
        let root = temp.path().to_path_buf();
        fs::create_dir_all(root.join(".local-tests")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        let file = root.join(".local-tests/client.cnf");
        fs::write(&file, "[client]\nhost=127.0.0.1\nport=3306\nuser=fixture-user\npassword=private-db-value\nssl-mode=REQUIRED\n").unwrap();
        let mut primary = json!({"host":"127.0.0.1","port":3306,"database":"source_control",
            "username":"fixture-user","password":"private-db-value","tls_mode":"required"});
        let mut targets = Vec::new();
        let mut databases = Vec::new();
        for (key, mode) in [
            ("shared-control", "shared"),
            ("shared", "shared"),
            ("dedicated-a", "dedicated"),
            ("dedicated-b", "dedicated"),
        ] {
            let database = format!("source_{}", key.replace('-', "_"));
            if key == "shared-control" {
                primary["database"] = json!(database);
            } else {
                let mut target = primary.clone();
                target["database"] = json!(database);
                target["key"] = json!(key);
                target["kind"] = json!("mysql");
                target["mode"] = json!(mode);
                targets.push(target);
            }
            databases.push(json!({"key":key,"kind":if key=="shared-control" {"combined"} else {"tenant"},"mode":mode,
                "database":database,"defaults_file":file,"defaults_sha256":digest::file(&file).unwrap().sha256,"server_uuid":"server-source"}));
        }
        let table = json!({"scope_id":"source-fixture","database":{"primary":primary},
            "tenant_data":{"targets":targets},"object_storage":{"backend":"rustfs","endpoint":"http://127.0.0.1:29000",
                "region":"us-east-1","access_key":"private-access-value","secret_key":"private-secret-value","use_ssl":false}});
        let source = json!({"scope_id":"source-fixture","databases":databases,
            "s3":{"endpoint":"http://127.0.0.1:29000","region":"us-east-1","access_key":"private-access-value","secret_key":"private-secret-value"}});
        Self {
            _temp: temp,
            root,
            table,
            source,
        }
    }
    fn args(&self) -> Value {
        let mut source = self.source.clone();
        source["s3"] = json!({"configuration": app_configuration::configuration_value(
            &self.root, &self.root.join("config"), "test").unwrap()});
        json!({"backend":self.root,"evidence_root":self.root,"source":source,"app_env":"test","config_dir":null})
    }
    fn write(&self) {
        fs::write(
            self.root.join("config/app.test.toml"),
            table_text(&self.table),
        )
        .unwrap();
    }
    fn verify(&self) -> Result<Value> {
        physical_table(&self.root, &self.source, &self.table)
    }
}

#[test]
fn public_four_database_binding_is_sorted_canonical_and_contains_no_credentials() {
    let mut f = Fixture::new();
    let expected = f.verify().unwrap();
    f.source["databases"].as_array_mut().unwrap().reverse();
    assert_eq!(f.verify().unwrap(), expected);
    for secret in [
        "private-db-value",
        "private-access-value",
        "private-secret-value",
        "fixture-user",
        "client.cnf",
    ] {
        assert!(!expected.to_string().contains(secret));
    }
    let mut public = expected.clone();
    public.as_object_mut().unwrap().remove("sha256");
    assert_eq!(expected["sha256"], digest::plan_hash(&public).unwrap());
}

#[test]
fn every_primary_and_tenant_connection_component_and_type_is_authoritative() {
    let f = Fixture::new();
    for location in ["/database/primary", "/tenant_data/targets/0"] {
        for (field, value) in [
            ("host", json!("::1")),
            ("port", json!(3307)),
            ("port", json!(true)),
            ("port", json!(0)),
            ("port", json!(65536)),
            ("port", json!("3306")),
            ("database", json!("other")),
            ("username", json!("other")),
            ("password", json!("wrong-secret")),
            ("tls_mode", json!("disabled")),
            ("password_env", json!("legacy")),
            ("tls_ca", json!("unbound.pem")),
            ("tls_client_cert", json!("unbound.pem")),
            ("tls_client_key", json!("unbound.pem")),
        ] {
            let mut table = f.table.clone();
            table.pointer_mut(location).unwrap()[field] = value;
            let error = physical_table(&f.root, &f.source, &table)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("wrong-secret"));
        }
        for field in ["password", "port", "tls_mode"] {
            let mut table = f.table.clone();
            table
                .pointer_mut(location)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(physical_table(&f.root, &f.source, &table).is_err());
        }
    }
}

#[test]
fn duplicates_missing_targets_aliases_and_unregistered_sources_are_rejected() {
    let f = Fixture::new();
    for field in ["replicas", "sources"] {
        for value in [json!([{}]), json!({}), json!(null), json!("secret-invalid")] {
            let mut table = f.table.clone();
            table["database"][field] = value;
            assert!(physical_table(&f.root, &f.source, &table).is_err());
        }
    }
    let mut table = f.table.clone();
    table["tenant_data"]["targets"][1] = table["tenant_data"]["targets"][0].clone();
    assert!(physical_table(&f.root, &f.source, &table).is_err());
    let mut source = f.source.clone();
    source["databases"][2]["database"] = source["databases"][1]["database"].clone();
    let mut table = f.table.clone();
    table["tenant_data"]["targets"][1]["database"] = source["databases"][1]["database"].clone();
    assert!(physical_table(&f.root, &source, &table).is_err());
    source = f.source.clone();
    source["databases"][0]["kind"] = json!("tenant");
    assert!(physical_table(&f.root, &source, &f.table).is_err());
    source = f.source.clone();
    source["databases"].as_array_mut().unwrap().pop();
    assert!(physical_table(&f.root, &source, &f.table).is_err());
}

#[test]
fn object_endpoint_region_protocol_backend_and_plaintext_keys_must_match() {
    let f = Fixture::new();
    for (field, value) in [
        ("endpoint", json!("http://127.0.0.1:29001")),
        ("region", json!("us-west-2")),
        ("use_ssl", json!(true)),
        ("backend", json!("local")),
        ("access_key", json!("wrong-secret")),
        ("secret_key", json!("wrong-secret")),
        ("secret_key_env", json!("legacy")),
    ] {
        let mut table = f.table.clone();
        table["object_storage"][field] = value;
        assert!(physical_table(&f.root, &f.source, &table).is_err());
    }
}

#[test]
fn explicit_configuration_root_and_evidence_root_are_independent_and_reloaded() {
    let f = Fixture::new();
    f.write();
    let mut args = f.args();
    let expected = physical(&args).unwrap();
    let execution = f.root.join("execution");
    fs::create_dir(&execution).unwrap();
    args["backend"] = json!(execution);
    args["config_dir"] = json!(f.root.join("config"));
    assert_eq!(physical(&args).unwrap(), expected);
    args["evidence_root"] = args["backend"].clone();
    assert!(physical(&args).is_err());
    args = f.args();
    fs::write(
        f.root.join("config/app.test.toml"),
        "scope_id='secret-broken'\n[database invalid",
    )
    .unwrap();
    assert!(
        !physical(&args)
            .unwrap_err()
            .to_string()
            .contains("secret-broken")
    );
    f.write();
    let mut table = f.table.clone();
    table["scope_id"] = json!("other");
    fs::write(f.root.join("config/app.test.toml"), table_text(&table)).unwrap();
    assert!(physical(&args).is_err());
    for env in [
        json!("dev"),
        json!("prod"),
        json!(" test"),
        json!(null),
        json!(true),
    ] {
        args["app_env"] = env;
        assert!(physical(&args).is_err());
    }
}

#[test]
fn generation_v1_selects_the_historical_execution_backend_configuration_read_only() {
    use crate::recovery::clone::native::target_verify::input::request_configuration::canonical_path;

    let f = Fixture::new();
    f.write();
    let coordinator = f.root.join("config");
    let execution = f.root.join("historical-execution");
    let directory = execution.join("config");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("app.toml"), "# 历史公共配置\n").unwrap();
    fs::write(
        directory.join("app.test.toml"),
        "scope_id='historical-execution'\n",
    )
    .unwrap();
    let snapshot = |config: &Path| {
        fs::read_dir(config)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect::<BTreeMap<_, _>>()
    };
    let before = [snapshot(&coordinator), snapshot(&directory)];
    assert_ne!(before[0], before[1]);

    let request = json!({"format_version":1,"kind":"devex-clone-seed-source-generation","id":"fixed-request",
        "source_registration":{"path":"registration"},"source_rebind":{"path":"rebind"},
        "review_successor":{"path":"successor"},"current_storage":{"attempt":65},
        "execution_backend":execution,"expected_backend_sha":"a".repeat(40),
        "adapter_contract":null,"product_backend":null,"backend_build":{"path":"old-runtime"},
        "maintenance_build":{"path":"old-maintenance"},"source_environment":{"path":"environment"}});
    let envelope = json!({"operation":"generation-request-configuration","write":false,
        "arguments":{"backend":f.root,"request":request}});
    let binding = super::super::execute(&serde_json::to_vec(&envelope).unwrap()).unwrap();
    assert_eq!(
        binding["directory"],
        json!(canonical_path(&directory).unwrap())
    );
    assert_eq!(binding["app_env"], "test");
    assert_eq!([snapshot(&coordinator), snapshot(&directory)], before);
}

#[test]
fn generation_v2_consumes_bound_sidecar_configuration_and_freezes_absent_common_file() {
    use crate::recovery::source::evidence::generation_request::configuration;
    let f = Fixture::new();
    f.write();
    let directory = f.root.join(".local-tests/bootstrap/config");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("app.test.toml"), table_text(&f.table)).unwrap();
    let frozen = configuration::capture(&f.root, &directory).unwrap();
    let mut request = json!({"format_version":2,"kind":"devex-clone-seed-source-generation","id":"source-config-test",
        "source_registration":null,"review_successor":null,"source_rebind":null,"current_storage":null,
        "execution_backend":f.root.join("clean-execution"),"expected_backend_sha":"a".repeat(40),
        "adapter_contract":null,"product_backend":null,"backend_build":null,"maintenance_build":null,
        "source_environment":null,"configuration":frozen,"execution_source":null});
    fs::create_dir(f.root.join("clean-execution")).unwrap();
    assert!(!f.root.join("clean-execution/config").exists());
    assert_eq!(
        configuration::directory(&f.root, &request).unwrap(),
        directory
    );
    assert!(frozen["files"].get("app.toml").is_none());
    let mut args = f.args();
    let expected = physical(&args).unwrap();
    args["backend"] = request["execution_backend"].clone();
    args["config_dir"] = json!(directory);
    args["source"]["s3"]["configuration"] = frozen;
    assert_eq!(physical(&args).unwrap(), expected);
    fs::write(directory.join("app.toml"), "scope_id='source-fixture'\n").unwrap();
    assert!(configuration::verify(&f.root, &request).is_err());
    fs::remove_file(directory.join("app.toml")).unwrap();
    configuration::verify(&f.root, &request).unwrap();
    fs::write(directory.join("app.test.toml"), "scope_id='changed'\n").unwrap();
    assert!(configuration::verify(&f.root, &request).is_err());
    request["configuration"]["app_env"] = json!("dev");
    assert!(configuration::verify(&f.root, &request).is_err());
    let retired = std::collections::BTreeMap::from([
        ("APP_ENV".into(), "test".into()),
        ("APP_CONFIG_DIR".into(), "retired".into()),
    ]);
    assert!(crate::recovery::source::evidence::environment::configured(&retired).is_err());
}

#[test]
fn defaults_parsing_rejects_ambiguity_changes_external_files_and_never_exposes_password() {
    let f = Fixture::new();
    let file = path(&f.source["databases"][0]["defaults_file"]).unwrap();
    let original = fs::read_to_string(&file).unwrap();
    for raw in [
        original.clone() + "ssl-ca=unbound.pem\n",
        original.replace("ssl-mode=REQUIRED\n", ""),
        original.replace("private-db-value", "\"private db value\""),
        original.replace("private-db-value", "private-db-value#comment"),
    ] {
        fs::write(&file, raw).unwrap();
        let mut source = f.source.clone();
        for db in source["databases"].as_array_mut().unwrap() {
            db["defaults_sha256"] = json!(digest::file(&file).unwrap().sha256);
        }
        assert!(physical_table(&f.root, &source, &f.table).is_err());
    }
    fs::write(
        &file,
        original.replace("private-db-value", "\"private-db-value\""),
    )
    .unwrap();
    let mut source = f.source.clone();
    for db in source["databases"].as_array_mut().unwrap() {
        db["defaults_sha256"] = json!(digest::file(&file).unwrap().sha256);
    }
    assert!(physical_table(&f.root, &source, &f.table).is_ok());
    assert!(f.verify().is_err());
    let summary = summary(&json!({"backend":f.root,"database":source["databases"][0]})).unwrap();
    assert!(summary.get("password").is_none());
}

#[test]
fn native_diagnostics_remove_raw_url_json_and_unicode_password_variants_without_returning_secrets()
{
    let f = Fixture::new();
    let file = path(&f.source["databases"][0]["defaults_file"]).unwrap();
    fs::write(&file,"[client]\nhost=127.0.0.1\nport=3306\nuser=fixture-user\npassword=密码 Aa1!~\nssl-mode=REQUIRED\n").unwrap();
    let mut db = f.source["databases"][0].clone();
    db["defaults_sha256"] = json!(digest::file(&file).unwrap().sha256);
    let value=redact(&json!({"backend":f.root,"databases":[db],
        "text":"密码 Aa1!~ %E5%AF%86%E7%A0%81%20Aa1%21~ %E5%AF%86%E7%A0%81+Aa1%21~ \\u5bc6\\u7801 Aa1!~"})).unwrap();
    assert_eq!(value, "[REDACTED] [REDACTED] [REDACTED] [REDACTED]");
}

#[test]
fn native_configuration_inserts_plaintext_only_into_create_new_bound_file() {
    let f = Fixture::new();
    let mut table = f.table.clone();
    for target in std::iter::once(table.pointer_mut("/database/primary").unwrap()) {
        target.as_object_mut().unwrap().remove("username");
        target.as_object_mut().unwrap().remove("password");
    }
    for target in table["tenant_data"]["targets"].as_array_mut().unwrap() {
        target.as_object_mut().unwrap().remove("username");
        target.as_object_mut().unwrap().remove("password");
    }
    let directory = f.root.join(".local-tests/config");
    fs::create_dir(&directory).unwrap();
    let output = directory.join("app.test.toml");
    let args = json!({"backend":f.root,"database":f.source["databases"][0],"output":output,"text":table_text(&table)});
    let mut bad = args.clone();
    bad["text"] = json!(table_text(&{
        let mut value = table.clone();
        value["tenant_data"]["targets"][0]["port"] = json!(3307);
        value
    }));
    assert!(configuration(&bad).is_err());
    assert!(!output.exists());
    let mut bad = args.clone();
    bad["text"] = json!(table_text(&{
        let mut value = table.clone();
        value["database"]["primary"]["password_env"] = json!("IGNORED");
        value
    }));
    assert!(configuration(&bad).is_err());
    assert!(!output.exists());
    let result = configuration(&args).unwrap();
    assert!(!result.to_string().contains("private-db-value"));
    let raw = fs::read_to_string(&output).unwrap();
    let written: toml::Table = raw.parse().unwrap();
    assert_eq!(
        written["database"]["primary"]["password"].as_str(),
        Some("private-db-value")
    );
    for target in written["tenant_data"]["targets"].as_array().unwrap() {
        assert_eq!(target["password"].as_str(), Some("private-db-value"));
    }
    assert!(configuration(&args).is_err());
    assert_eq!(fs::read_to_string(output).unwrap(), raw);
}

#[test]
fn explicit_control_empty_reads_disabled_tls_and_https_storage_are_supported() {
    let mut f = Fixture::new();
    f.table["tenant_data"]["targets"]
        .as_array_mut()
        .unwrap()
        .push(json!({"key":"shared-control","kind":"control","mode":"shared"}));
    f.table["database"]["replicas"] = json!([]);
    f.table["database"]["sources"] = json!([]);
    assert!(f.verify().is_ok());
    let defaults = path(&f.source["databases"][0]["defaults_file"]).unwrap();
    let raw = fs::read_to_string(&defaults)
        .unwrap()
        .replace("REQUIRED", "DISABLED");
    fs::write(&defaults, raw).unwrap();
    for db in f.source["databases"].as_array_mut().unwrap() {
        db["defaults_sha256"] = json!(digest::file(&defaults).unwrap().sha256);
    }
    f.table["database"]["primary"]["tls_mode"] = json!("disabled");
    for target in f.table["tenant_data"]["targets"].as_array_mut().unwrap() {
        if target["kind"] == "mysql" {
            target["tls_mode"] = json!("disabled");
        }
    }
    f.source["s3"]["endpoint"] = json!("https://127.0.0.1:29000/");
    f.table["object_storage"]["endpoint"] = json!("https://127.0.0.1:29000");
    f.table["object_storage"]["use_ssl"] = json!(true);
    assert!(f.verify().is_ok());
    for field in [
        "host",
        "database",
        "username",
        "password",
        "port",
        "tls_mode",
        "max_connections",
        "password_env",
    ] {
        let mut table = f.table.clone();
        let last = table["tenant_data"]["targets"].as_array().unwrap().len() - 1;
        table["tenant_data"]["targets"][last][field] = json!("unbound");
        assert!(
            physical_table(&f.root, &f.source, &table).is_err(),
            "{field}"
        );
    }
}

#[test]
fn loopback_aliases_cannot_mask_same_server_and_database_but_distinct_servers_can_share_names() {
    let mut f = Fixture::new();
    let original = path(&f.source["databases"][0]["defaults_file"]).unwrap();
    let alternate = f.root.join(".local-tests/alternate.cnf");
    fs::write(
        &alternate,
        fs::read_to_string(original)
            .unwrap()
            .replace("127.0.0.1", "::1"),
    )
    .unwrap();
    let database = f.source["databases"][1]["database"].clone();
    f.source["databases"][2]["database"] = database.clone();
    f.source["databases"][2]["defaults_file"] = json!(alternate);
    f.source["databases"][2]["defaults_sha256"] = json!(digest::file(&alternate).unwrap().sha256);
    f.table["tenant_data"]["targets"][1]["host"] = json!("::1");
    f.table["tenant_data"]["targets"][1]["database"] = database;
    assert!(f.verify().is_err());
    f.source["databases"][2]["server_uuid"] = json!("distinct-server");
    assert!(f.verify().is_ok());
}
