use super::*;
use model::Input;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

fn temporary() -> tempfile::TempDir {
    let root = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn request(root: &Path) -> Value {
    let source = root.join("input");
    let output = root.join("output");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&output).unwrap();
    json!({"format_version":1,"backend":root,"output":output,"config_dir":source,
        "github_actions":"true","run_id":"42","attempt":"3","app_env":"test",
        "container":"a".repeat(64),"settings":{"scope_id":"ci-42-3","snowflake_worker_id":3,
            "database":{"primary":binding()},"tenant_data":{}},
        "common_text":"[tenant_data]\n",
        "environment_text":"scope_id = 'ci-42-3'\nsnowflake_worker_id = 3\n[database.primary]\nhost='127.0.0.1'\nport=3306\ndatabase='ryframe_e2e_42_3'\nusername='root'\npassword='private-password'\ntls_mode='required'\n"})
}

fn binding() -> Value {
    json!({"host":"127.0.0.1","port":3306,"database":"ryframe_e2e_42_3",
        "username":"root","password":"private-password","tls_mode":"required"})
}

fn fixture() -> &'static Path {
    static CLIENT: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &CLIENT
        .get_or_init(|| {
            let root = temporary();
            let path = root
                .path()
                .join(format!("mysql 替身{}", std::env::consts::EXE_SUFFIX));
            let mut command = Command::new("rustc");
            command
                .args([
                    "--edition",
                    "2024",
                    "-C",
                    "debuginfo=0",
                    "--crate-name",
                    "ci_database_client",
                ])
                .arg(
                    crate::workspace::root_dir().join("xtask/tests/fixtures/ci_database_client.rs"),
                )
                .arg("-o")
                .arg(&path);
            if cfg!(windows) {
                command.args(["-C", "linker=rust-lld.exe"]);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            (root, path)
        })
        .1
}

#[test]
fn plan_is_read_only_public_and_exactly_four_schemas() {
    let root = temporary();
    let value = request(root.path());
    let input: Input = serde_json::from_value(value).unwrap();
    let plan = input.validate().unwrap();
    assert_eq!(
        plan.schemas,
        [
            "ryframe_e2e_42_3",
            "ryframe_e2e_42_3_shared",
            "ryframe_e2e_42_3_dedicated_a",
            "ryframe_e2e_42_3_dedicated_b"
        ]
    );
    assert!(!plan.public().to_string().contains("private-password"));
    assert_eq!(fs::read_dir(root.path().join("output")).unwrap().count(), 0);
}

#[test]
fn invalid_context_paths_and_instances_fail_without_writes() {
    let root = temporary();
    let value = request(root.path());
    for (field, invalid) in [
        ("github_actions", json!("false")),
        ("app_env", json!("prod")),
        ("run_id", json!("42;DROP")),
        ("attempt", json!("0")),
        ("attempt", json!("1000000")),
        ("container", json!("bad;command")),
        ("output", json!("relative")),
        ("format_version", json!(2)),
    ] {
        let mut changed = value.clone();
        changed[field] = invalid;
        let input: Input = serde_json::from_value(changed).unwrap();
        assert!(input.validate().is_err(), "{field}");
    }
    for invalid in [
        json!(2),
        json!(true),
        json!(-1),
        json!(1024),
        json!("3"),
        Value::Null,
    ] {
        let mut changed = value.clone();
        changed["settings"]["snowflake_worker_id"] = invalid;
        let input: Input = serde_json::from_value(changed).unwrap();
        assert_eq!(input.validate().err(), Some("instance_id"));
    }
    assert_eq!(fs::read_dir(root.path().join("output")).unwrap().count(), 0);
}

#[test]
fn invalid_mysql_fields_and_tenant_shapes_are_rejected_and_redacted() {
    for (key, value) in [
        ("host", json!("remote.example")),
        ("database", json!("bad;name")),
        ("port", json!(true)),
        ("port", json!(0)),
        ("port", json!(65536)),
        ("port", json!("3306x")),
        ("username", json!("")),
        ("password", json!("")),
        ("password", json!("secret\0")),
        ("password", json!("secret\u{7}")),
        ("password_env", json!("PRIVATE")),
        ("tls_mode", json!("preferred")),
        ("tls_mode", json!("verify_identity")),
        ("tls_ca", json!(true)),
        ("tls_ca", json!("ca\npath")),
        ("tls_client_cert", json!("cert")),
    ] {
        let mut changed = binding();
        changed[key] = value;
        let error = model::validate_binding(&changed).unwrap_err();
        assert_eq!(error, "binding_invalid", "{key}");
        assert!(!response(Err(error)).1.to_string().contains("PRIVATE"));
    }
    let root = temporary();
    let value = request(root.path());
    for (key, invalid) in [
        ("tenant_data", json!({"targets":[]})),
        ("tenant_data", json!("private-value")),
        ("database", json!({"primary":"private-value"})),
    ] {
        let mut changed = value.clone();
        changed["settings"][key] = invalid;
        assert!(
            serde_json::from_value::<Input>(changed)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}

#[test]
fn verified_tls_container_scope_port_and_credentials_are_preserved() {
    let root = temporary();
    let mut binding = binding();
    binding["port"] = json!(19210);
    binding["password"] = json!("中文🔐\"\\line\nnext\r\t\u{8}");
    binding["tls_mode"] = json!("verify_identity");
    binding["tls_ca"] = json!("/tmp/ci ca.pem");
    binding["tls_client_cert"] = json!("C:/测试/cert.pem");
    binding["tls_client_key"] = json!("C:/测试/key.pem");
    let invocation = client::invocation(
        &binding,
        client::Client::Container(&"a".repeat(64)),
        root.path(),
    )
    .unwrap();
    let args = invocation
        .command
        .get_args()
        .map(|v| v.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(&args[..3], ["exec", "-i", &"a".repeat(64)]);
    for flag in [
        "--port=3306",
        "--ssl-mode=VERIFY_IDENTITY",
        "--ssl-ca=/tmp/ci ca.pem",
        "--ssl-cert=C:/测试/cert.pem",
        "--ssl-key=C:/测试/key.pem",
    ] {
        assert!(args.iter().any(|v| v == flag));
    }
    assert!(!args.join(" ").contains("中文"));
    assert_eq!(
        String::from_utf8(invocation.prefix.clone()).unwrap(),
        "[client]\npassword=\"中文🔐\\\"\\\\line\\nnext\\r\\t\\b\"\n"
    );
    assert_eq!(
        fs::read(invocation._options.path()).unwrap(),
        invocation.prefix
    );
    assert!(
        !invocation
            .command
            .get_envs()
            .any(|(k, v)| v.is_some()
                && (k.to_string_lossy().starts_with("APP_") || k == "MYSQL_PWD"))
    );
    let options = invocation._options.path().to_owned();
    drop(invocation);
    assert!(!options.exists());
}

#[test]
fn controls_reject_wrong_types_extra_keys_and_relative_native_paths() {
    let root = temporary();
    fs::create_dir(root.path().join(".local-tests")).unwrap();
    let path = root.path().join(".local-tests/full-stack-control.json");
    let value = json!({"format_version":1,"runtime_dir":root.path().join("runtime"),"mysql_client":fixture()});
    fs::write(&path, value.to_string()).unwrap();
    assert_eq!(model::control(root.path()).unwrap(), value);
    for (key, invalid) in [
        ("format_version", json!(true)),
        ("runtime_dir", json!("relative")),
        ("mysql_client", json!("relative/mysql")),
        ("extra", json!("private-value")),
    ] {
        let mut changed = value.clone();
        changed[key] = invalid;
        fs::write(&path, changed.to_string()).unwrap();
        assert_eq!(model::control(root.path()).err(), Some("control_invalid"));
    }
    fs::write(
        &path,
        format!(
            "{{\"format_version\":1,\"format_version\":1,\"runtime_dir\":{}}}",
            json!(root.path())
        ),
    )
    .unwrap();
    assert!(model::control(root.path()).is_err());
    fs::remove_file(path).unwrap();
    assert_eq!(model::control(root.path()).err(), Some("control_invalid"));
}

#[test]
fn protocol_is_bounded_strict_and_never_echoes_private_values() {
    for input in [
        br#"{"private":"private-password"}"#.as_slice(),
        br#"{"backend":"relative","backend":"other"}"#,
        b"not json",
    ] {
        let (code, value) = response(dispatch("control", input));
        assert_eq!(code, 1);
        assert!(!value.to_string().contains("private-password"));
    }
    assert_eq!(dispatch("unknown", b"{}").err(), Some("protocol_operation"));
}

#[test]
fn instance_id_rewrite_only_changes_top_level_for_all_key_spellings() {
    for key in [
        "snowflake_worker_id",
        "'snowflake_worker_id'",
        "\"snowflake_worker_id\"",
    ] {
        let text = format!("{key}=3\n[app]\nsnowflake_worker_id=99\n");
        let rendered = prepare::instance_text(&text, 2).unwrap();
        let value: toml::Table = toml::from_str(&rendered).unwrap();
        assert_eq!(value["snowflake_worker_id"].as_integer(), Some(2));
        assert_eq!(value["app"]["snowflake_worker_id"].as_integer(), Some(99));
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod process_tests;

#[test]
fn file_protocol_rejects_unknown_fields_relative_paths_and_replay() {
    let root = temporary();
    let path = root.path().join("private");
    assert!(dispatch("private-directory", br#"{"path":"relative"}"#).is_err());
    let input = serde_json::to_vec(&json!({"path":path,"extra":"private-password"})).unwrap();
    assert!(dispatch("private-directory", &input).is_err());
    assert!(!path.exists());
    let input = serde_json::to_vec(&json!({"path":path})).unwrap();
    assert_eq!(
        dispatch("private-directory", &input).unwrap()["state"],
        "created"
    );
    assert!(dispatch("private-directory", &input).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[test]
fn instance_protocol_validates_type_range_and_syntax_without_exposing_secrets() {
    for id in [json!(true), json!(-1), json!(1024), json!("2")] {
        let input =
            serde_json::to_vec(&json!({"text":"password='private-password'","worker_id":id}))
                .unwrap();
        let (code, value) = response(dispatch("instance-text", &input));
        assert_eq!(code, 1);
        assert!(!value.to_string().contains("private-password"));
    }
    let input = serde_json::to_vec(
        &json!({"text":"snowflake_worker_id=3\n[app]\nsnowflake_worker_id=99\n","worker_id":2}),
    )
    .unwrap();
    let result = dispatch("instance-text", &input).unwrap();
    let table: toml::Table = toml::from_str(result["text"].as_str().unwrap()).unwrap();
    assert_eq!(table["snowflake_worker_id"].as_integer(), Some(2));
    assert_eq!(table["app"]["snowflake_worker_id"].as_integer(), Some(99));
    let input =
        serde_json::to_vec(&json!({"text":"password='private-password'\n[broken","worker_id":2}))
            .unwrap();
    let (_, value) = response(dispatch("instance-text", &input));
    assert!(!value.to_string().contains("private-password"));
}
