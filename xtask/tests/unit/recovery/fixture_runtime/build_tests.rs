use super::*;
use crate::recovery::full_stack::build::tests::Fixture;
use std::{fs, path::PathBuf};

struct Prepared {
    fixture: Fixture,
    environment: PathBuf,
    output: PathBuf,
}

impl Prepared {
    fn new() -> Self {
        let fixture = Fixture::new();
        let root = &fixture.context.root;
        fs::write(root.join(".git/info/exclude"), ".local-tests/\n").unwrap();
        fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        let local = root.join(".local-tests/reference-fixture");
        let environment = local.join("environment");
        fs::create_dir_all(environment.join("config")).unwrap();
        let private = json!({"APP_ENV":"test","TEMP":local,"TMP":local});
        write_new(
            &environment.join("environment.json"),
            &json!({"environment":private}),
        )
        .unwrap();
        let mut secrets = serde_json::Map::new();
        for name in super::super::runtime::SECRET_FILES {
            let path = environment.join(name);
            fs::write(&path, format!("SyntheticSecret-{name}")).unwrap();
            secrets.insert((*name).into(), bound(&path).unwrap());
        }
        write_new(
            &environment.join("bootstrap.json"),
            &json!({
                "format_version":1,"kind":"reference-fixture-environment","status":"prepared",
                "services_started":false,"remote_writes":0,"historical_data_used":false,
                "execution_backend":root,"environment_sha256":digest::plan_hash(&private).unwrap(),
                "secret_files":secrets
            }),
        )
        .unwrap();
        fs::write(environment.join("config/app.toml"), "# shared defaults\n").unwrap();
        fs::write(environment.join("config/app.test.toml"),
            "scope_id = 'fixture-build'\nsnowflake_worker_id = 1\n[app]\nhost = '127.0.0.1'\nport = 18200\n[jobs]\nmode = 'external'\nhealth_host = '127.0.0.1'\nhealth_port = 19200\n").unwrap();
        fixture.business();
        let output = local.join("runtime-build");
        Self {
            fixture,
            environment,
            output,
        }
    }

    fn inputs(&self) -> Inputs {
        Inputs::load(
            &crate::workspace::root_dir(),
            &self.environment.join("bootstrap.json"),
        )
        .unwrap()
    }

    fn command(&self, program: &str, args: &[String]) -> Result<String> {
        match (program, args.first().map(String::as_str)) {
            ("cargo", Some("-V")) => Ok("cargo synthetic\n".into()),
            ("rustc", Some("-vV")) => Ok("rustc synthetic\nhost: x86_64-pc-windows-msvc\n".into()),
            ("cargo", Some("build")) => {
                let name = &args[args.iter().position(|s| s == "--bin").unwrap() + 1];
                let executable = self.fixture.context.output.join(format!("{name}.exe"));
                fs::write(&executable, name)?;
                Ok(json!({"reason":"compiler-artifact",
                    "manifest_path":self.fixture.context.root.join("crates/ryframe/Cargo.toml"),
                    "target":{"name":name,"kind":["bin"]},"executable":executable})
                .to_string())
            }
            _ => panic!("构建不调用解释器或服务"),
        }
    }
}

#[test]
fn native_fixture_build_registers_dirty_source_four_artifacts_and_sidecar_configuration() {
    let prepared = Prepared::new();
    let inputs = prepared.inputs();
    let value = build(&inputs, &prepared.output, &mut |program, args| {
        prepared.command(program, args)
    })
    .unwrap();
    assert_eq!(value["status"], "reference_fixture_runtime_built");
    assert_eq!(value["remote_writes"], 0);
    let receipt =
        fixture_control::verify_full_stack_runtime(&inputs.root, &prepared.output).unwrap();
    let sidecar = prepared
        .output
        .parent()
        .unwrap()
        .join("runtime-build-configuration");
    assert_eq!(receipt["api_config_dir"], json!(sidecar.join("config")));
    assert_eq!(
        receipt["worker_config_dir"],
        json!(sidecar.join("worker-config"))
    );
    for (role, id) in [("config", 1), ("worker-config", 2)] {
        let destination = sidecar.join(role);
        assert_eq!(
            fs::read(destination.join("app.toml")).unwrap(),
            fs::read(inputs.configuration.join("app.toml")).unwrap()
        );
        let table: toml::Table =
            toml::from_str(&fs::read_to_string(destination.join("app.test.toml")).unwrap())
                .unwrap();
        assert_eq!(table["snowflake_worker_id"].as_integer(), Some(id));
    }
    let backend = document(&prepared.output.join("backend-build.json"))
        .unwrap()
        .0;
    let expected = source::backend_domains(&source::capture(&inputs.root).unwrap()).unwrap();
    assert_eq!(backend["sources"], expected);
    assert!(
        backend["build"]["environment"]["variables"]
            .as_array()
            .unwrap()
            .iter()
            .all(|name| name.as_str().is_some_and(|name| [
                "PATH",
                "CARGO_TARGET_DIR",
                "CARGO_BUILD_JOBS"
            ]
            .iter()
            .any(|allowed| name.eq_ignore_ascii_case(allowed))))
    );
    bridge::verify_build(
        &inputs.root,
        &prepared.output.join("backend-build.json"),
        &backend,
    )
    .unwrap();
    let binaries = document(&prepared.output.join("binaries.json")).unwrap().0;
    for name in [
        "ryframe",
        "ryframe-worker",
        "ryframe-reset",
        "ryframe-migrate",
    ] {
        assert!(Path::new(binaries[name].as_str().unwrap()).is_file());
    }
    assert!(
        build(&inputs, &prepared.output, &mut |_, _| panic!(
            "已有输出不得重放"
        ))
        .is_err()
    );
}

#[test]
fn native_fixture_build_retains_partial_failure_and_rejects_source_or_configuration_drift() {
    for failure in ["command", "source", "configuration", "artifact"] {
        let prepared = Prepared::new();
        let inputs = prepared.inputs();
        let mut changed = false;
        let result = build(&inputs, &prepared.output, &mut |program, args| {
            if args.first().map(String::as_str) == Some("build") && !changed {
                changed = true;
                match failure {
                    "command" => return Err("合成 Cargo 失败".into()),
                    "source" => fs::write(inputs.root.join("README.md"), "changed")?,
                    "configuration" => fs::write(inputs.configuration.join("app.toml"), "# changed")?,
                    "artifact" => return Ok(json!({"reason":"compiler-artifact","target":{"name":"ryframe-reset","kind":["lib"]},
                        "manifest_path":inputs.root.join("crates/ryframe/Cargo.toml"),"executable":prepared.fixture.context.root.join("README.md")}).to_string()),
                    _ => unreachable!(),
                }
            }
            prepared.command(program, args)
        });
        assert!(result.is_err(), "{failure}");
        assert!(!prepared.output.join("runtime.json").exists());
        let failure_path = prepared.output.join("failure.json");
        let original = fs::read(&failure_path).unwrap();
        assert_eq!(
            document(&failure_path).unwrap().0["status"],
            "reference_fixture_runtime_failed"
        );
        assert!(
            build(&inputs, &prepared.output, &mut |_, _| panic!(
                "失败输出不得重放"
            ))
            .is_err()
        );
        assert_eq!(fs::read(failure_path).unwrap(), original);
    }
}

#[test]
fn native_fixture_build_environment_preserves_cargo_target_without_application_or_proxy_overrides()
{
    let private = json!({"APP_ENV":"test","TEMP":"bound-temp","TMP":"bound-tmp"});
    let inherited = [
        ("PATH", "system-path"),
        ("CARGO_TARGET_DIR", "D:/shared-target"),
        ("CARGO_HOME", "unapproved-cargo-home"),
        ("CARGO_BUILD_TARGET", "unapproved-target"),
        ("CARGO_BUILD_JOBS", "1"),
        ("APP_ENV", "prod"),
        ("APP_SECRET", "SyntheticSecret"),
        ("HTTP_PROXY", "http://synthetic.invalid"),
        ("TEMP", "host-temp"),
        ("TMP", "host-tmp"),
    ]
    .map(|(key, value)| (key.into(), value.into()));
    let actual = environment::configured(&private, inherited).unwrap();
    let expected: std::collections::BTreeMap<String, String> = [
        ("PATH", "system-path"),
        ("CARGO_TARGET_DIR", "D:/shared-target"),
        ("CARGO_BUILD_JOBS", "1"),
        ("APP_ENV", "test"),
        ("TEMP", "bound-temp"),
        ("TMP", "bound-tmp"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    assert_eq!(actual, expected);
    let recorded = bridge::build_context::from_inputs(
        "cargo synthetic",
        "rustc synthetic\nhost: synthetic-target",
        &actual,
    )
    .unwrap();
    assert!(
        recorded["environment"]["variables"]
            .as_array()
            .unwrap()
            .contains(&json!("CARGO_TARGET_DIR"))
    );
    assert_eq!(recorded["jobs"], "1");
    assert!(
        recorded["environment"]["variables"]
            .as_array()
            .unwrap()
            .contains(&json!("CARGO_BUILD_JOBS"))
    );
    let mut changed = actual.clone();
    changed.insert("CARGO_BUILD_JOBS".into(), "2".into());
    let other = bridge::build_context::from_inputs(
        "cargo synthetic",
        "rustc synthetic\nhost: synthetic-target",
        &changed,
    )
    .unwrap();
    assert_eq!(other["jobs"], "2");
    assert_ne!(
        recorded["environment"]["sha256"],
        other["environment"]["sha256"]
    );
    let mut changed = actual;
    changed.insert("CARGO_TARGET_DIR".into(), "D:/other-target".into());
    let other = bridge::build_context::from_inputs(
        "cargo synthetic",
        "rustc synthetic\nhost: synthetic-target",
        &changed,
    )
    .unwrap();
    assert_ne!(
        recorded["environment"]["sha256"],
        other["environment"]["sha256"]
    );
    let absent = environment::configured(&private, []).unwrap();
    assert!(!absent.contains_key("CARGO_TARGET_DIR"));
    assert!(!absent.contains_key("CARGO_BUILD_JOBS"));
    let default = bridge::build_context::from_inputs(
        "cargo synthetic",
        "rustc synthetic\nhost: synthetic-target",
        &absent,
    )
    .unwrap();
    assert_eq!(default["jobs"], "cargo-default");
    for invalid in ["", "0", "000", "-1", "+1", "1.5", "one", " 1 "] {
        let inherited = [("CARGO_BUILD_JOBS".into(), invalid.into())];
        assert!(environment::configured(&private, inherited).is_err());
    }
}
