use super::*;
use crate::recovery::fixture_control::source_pair::execution::tests::Fixture;
use std::{fs, path::PathBuf};

fn bind_unlaunchable_python(root: &std::path::Path) -> PathBuf {
    let python = root.join(".local-tests/not-an-interpreter.exe");
    fs::write(&python, "synthetic executable; must never be launched").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&python, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(
        root.join(".local-tests/tools.toml"),
        format!("[verification]\npython_program = {}\n", json!(python)),
    )
    .unwrap();
    python
}

#[test]
fn native_browser_dependencies_use_bound_configuration_and_reject_frozen_input_drift() {
    use crate::recovery::fixture_runtime::dependency::{Dependencies, Native};
    let prepared = Prepared::new();
    let root = &prepared.context.root;
    let coordinator = &prepared.fixture.backend;
    let bootstrap = prepared.environment.join("bootstrap.json");
    let python = bind_unlaunchable_python(coordinator);
    let execution = root;
    let configuration = prepared.configuration.join("app.test.toml");
    let mut configured = fs::read_to_string(&configuration).unwrap();
    configured.push_str("[rate_limit]\ncapacity=37\nwindow_secs=2\napi_window_secs=19\n[rate_limit.api_limits]\n'POST /api/v1/auth/login'=5\n");
    fs::write(&configuration, &configured).unwrap();
    fs::write(
        prepared.configuration.join("app.dev.toml"),
        "[rate_limit]\ncapacity=41\nwindow_secs=3\n",
    )
    .unwrap();
    let mut receipt: Value = serde_json::from_slice(&fs::read(&bootstrap).unwrap()).unwrap();
    receipt["configuration_sha256"] = json!(
        fixture_control::artifact::runtime::configuration_digest(&prepared.configuration).unwrap()
    );
    fs::write(&bootstrap, serde_json::to_vec(&receipt).unwrap()).unwrap();

    let mut api = Native::new(coordinator).unwrap();
    let boot = json!({"backend":coordinator,"environment":bootstrap});
    let observed = api.call("bootstrap", boot.clone()).unwrap();
    assert_eq!(observed["python"], json!(python.canonicalize().unwrap()));
    assert_eq!(observed["config_dir"], json!(prepared.configuration));
    assert_ne!(prepared.configuration, prepared.environment.join("config"));
    let temporary = root.join(".local-tests/reference-fixture");
    assert_eq!(
        observed["private"],
        json!({"APP_ENV":"test","TEMP":temporary,"TMP":temporary})
    );
    let args = json!({"bootstrap":bootstrap,"execution":execution});
    let settings = api.call("settings", args).unwrap();
    assert_eq!(settings["scope_id"], "fixture-build");
    assert_eq!(settings["app"]["port"], 18200);
    let limits = api
        .call(
            "limits",
            json!({"bootstrap":bootstrap,"execution":execution,"app_env":"test"}),
        )
        .unwrap();
    assert_eq!(limits["capacity"], 37);
    assert_eq!(limits["api_limits"]["POST /api/v1/auth/login"], 5);
    let dev = api
        .call(
            "limits",
            json!({"bootstrap":bootstrap,"execution":execution,"app_env":"dev"}),
        )
        .unwrap();
    assert_eq!(dev["capacity"], 41);
    assert!(
        api.call(
            "limits",
            json!({"bootstrap":bootstrap,"execution":execution,"app_env":"prod"})
        )
        .is_err()
    );
    fs::write(
        &configuration,
        configured.replace("capacity=37", "capacity=38"),
    )
    .unwrap();
    assert!(
        api.call(
            "limits",
            json!({"bootstrap":bootstrap,"execution":execution,"app_env":"test"})
        )
        .is_err()
    );
    fs::write(&configuration, configured).unwrap();

    for file in [
        prepared.environment.join("environment.json"),
        prepared.fixture.backend.join(".local-tests/jwt-secret.txt"),
    ] {
        let original = fs::read(&file).unwrap();
        if file.file_name().unwrap() == "environment.json" {
            let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
            changed["environment"]["TEMP"] = json!(prepared.environment.join("changed-temp"));
            fs::write(&file, serde_json::to_vec(&changed).unwrap()).unwrap();
        } else {
            fs::write(&file, b"changed frozen input").unwrap();
        }
        assert!(api.call("bootstrap", boot.clone()).is_err());
        fs::write(&file, original).unwrap();
        assert!(api.call("bootstrap", boot.clone()).is_ok());
    }
}

#[test]
fn native_browser_tools_resolve_corepack_and_configured_node_without_running_them() {
    use crate::recovery::fixture_runtime::dependency::{Dependencies, Native};
    let temporary = super::super::tests::temporary();
    let root = temporary.path();
    fs::create_dir_all(root.join(".local-tests/corepack")).unwrap();
    let node = std::env::current_exe().unwrap().canonicalize().unwrap();
    fs::write(
        root.join(".local-tests/tools.toml"),
        format!("[verification]\nnode_program = {}\n", json!(node)),
    )
    .unwrap();
    let mut api = Native::new(root).unwrap();
    let tools = api.call("tools", json!({"frontend":root})).unwrap();
    let corepack = PathBuf::from(tools["corepack"].as_str().unwrap());
    assert!(corepack.is_absolute() && corepack.is_file());
    let home = std::env::var_os("COREPACK_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".local-tests/corepack"));
    assert_eq!(
        tools["corepack_home"],
        json!(root.join(home).canonicalize().unwrap())
    );
    if cfg!(windows) {
        assert_eq!(tools["node"], json!(node));
        assert!(
            corepack
                .parent()
                .unwrap()
                .join("node_modules/corepack/dist/corepack.js")
                .is_file()
        );
    } else {
        assert!(tools["node"].is_null());
    }
}

#[test]
fn native_browser_frontend_dependency_requires_current_complete_build_evidence() {
    use crate::recovery::{
        clone::native::source,
        fixture_runtime::dependency::{Dependencies, Native},
        frontend_build::tests as frontend,
    };
    let temporary = frontend::fixture();
    let root = temporary.path();
    let mut api = Native::new(root).unwrap();
    let inventory = source::inventory_value(&source::capture(root).unwrap());
    frontend::write_receipt(root, "development", &inventory);
    let proof = api
        .call("frontend-build", json!({"frontend":root}))
        .unwrap();
    assert_eq!(
        proof["path"],
        json!(root.join("dist/.vite/restore-build.json"))
    );
    fs::write(root.join("dist/index.html"), "late artifact drift").unwrap();
    assert!(
        api.call("frontend-build", json!({"frontend":root}))
            .is_err()
    );
}

struct Prepared {
    fixture: Fixture,
    context: full_stack::context::Context,
    environment: PathBuf,
    configuration: PathBuf,
    output: PathBuf,
}

impl Prepared {
    fn new() -> Self {
        let fixture = Fixture::new();
        let (_, plan) = fixture_control::environment::protocol::tests::published_plan(&fixture);
        let root = PathBuf::from(plan["execution_backend"].as_str().unwrap());
        let local = root.join(".local-tests/reference-fixture");
        let environment = local.join("environment");
        let configuration = local.join("selected-config");
        fs::create_dir_all(&environment).unwrap();
        fs::create_dir_all(&configuration).unwrap();
        fs::write(configuration.join("app.toml"), "# shared defaults\n").unwrap();
        fs::write(configuration.join("app.test.toml"),
            "scope_id = 'fixture-build'\nsnowflake_worker_id = 1\n[app]\nhost = '127.0.0.1'\nport = 18200\n[jobs]\nmode = 'external'\nhealth_host = '127.0.0.1'\nhealth_port = 19200\n").unwrap();
        let private = json!({"APP_ENV":"test","TEMP":local,"TMP":local});
        write_new(
            &environment.join("environment.json"),
            &json!({"environment":private}),
        )
        .unwrap();
        let mut secrets = serde_json::Map::new();
        for name in super::super::runtime::SECRET_FILES {
            // 私有文件属于 G；发布后的 P 不复制这组凭据。
            let path = fixture.backend.join(".local-tests").join(name);
            fs::write(&path, format!("SyntheticSecret-{name}")).unwrap();
            secrets.insert((*name).into(), bound(&path).unwrap());
        }
        write_new(
            &environment.join("bootstrap.json"),
            &json!({
                "format_version":1,"kind":"reference-fixture-environment","status":"prepared",
                "services_started":false,"remote_writes":0,"historical_data_used":false,
                "execution_backend":root,"environment_sha256":digest::plan_hash(&private).unwrap(),
                "secret_files":secrets,"plan":plan,"config_dir":configuration,
                "configuration_sha256":fixture_control::artifact::runtime::configuration_digest(&configuration).unwrap()
            }),
        ).unwrap();
        let artifacts = local.join("fake-cargo");
        fs::create_dir_all(&artifacts).unwrap();
        let context = full_stack::context::Context {
            root,
            output: artifacts,
            control: json!({}),
            run: String::new(),
            attempt: String::new(),
        };
        let output = local.join("runtime-build");
        Self {
            fixture,
            context,
            environment,
            configuration,
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
                let executable = self.context.output.join(format!("{name}.exe"));
                fs::write(&executable, name)?;
                Ok(json!({"reason":"compiler-artifact",
                    "manifest_path":self.context.root.join("crates/ryframe/Cargo.toml"),
                    "target":{"name":name,"kind":["bin"]},"executable":executable})
                .to_string())
            }
            _ => panic!("构建不调用解释器或服务"),
        }
    }
}

#[test]
fn native_fixture_build_registers_published_source_four_artifacts_and_explicit_configuration() {
    let prepared = Prepared::new();
    let inputs = prepared.inputs();
    let value = build(&inputs, &prepared.output, &mut |program, args| {
        prepared.command(program, args)
    })
    .unwrap();
    assert_eq!(value["status"], "reference_fixture_runtime_built");
    assert_eq!(value["remote_writes"], 0);
    let pair = document(&prepared.output.join("source-pair.json"))
        .unwrap()
        .0;
    assert_eq!(
        pair["kind"],
        fixture_control::source_pair::execution::PAIR_KIND
    );
    let provenance = full_stack::provenance::current(&inputs.root, &prepared.output).unwrap();
    for side in ["backend", "frontend"] {
        assert_ne!(
            provenance["original"][side]["head"],
            provenance["generated"][side]["head"]
        );
    }
    assert!(source::capture(&inputs.root).unwrap().source.snapshot.clean);

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
                "CARGO_BUILD_JOBS",
                "CARGO_HOME",
                "RUSTUP_HOME"
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
    assert_artifacts_survive_cache_reuse_and_reject_tampering(&prepared, &receipt, &backend);
    assert!(
        build(&inputs, &prepared.output, &mut |_, _| panic!(
            "已有输出不得重放"
        ))
        .is_err()
    );
}

fn assert_artifacts_survive_cache_reuse_and_reject_tampering(
    prepared: &Prepared,
    receipt: &Value,
    backend: &Value,
) {
    let binaries = document(&prepared.output.join("binaries.json")).unwrap().0;
    for name in [
        "ryframe",
        "ryframe-worker",
        "ryframe-reset",
        "ryframe-migrate",
    ] {
        let frozen = Path::new(binaries[name].as_str().unwrap());
        assert_eq!(
            frozen.parent(),
            Some(prepared.output.join("artifacts").as_path())
        );
        fs::write(
            prepared.context.output.join(format!("{name}.exe")),
            "later Cargo build",
        )
        .unwrap();
    }
    assert_eq!(
        fixture_control::verify_full_stack_runtime(&prepared.context.root, &prepared.output)
            .unwrap(),
        *receipt
    );
    bridge::verify_build(
        &prepared.context.root,
        &prepared.output.join("backend-build.json"),
        backend,
    )
    .unwrap();
    for path in [
        prepared.fixture.output.join("execution-source.json"),
        prepared.fixture.receipt.clone(),
        prepared
            .context
            .root
            .join("tools/python/fixtures/order-business/src/resources/mod.rs"),
        prepared.fixture.frontend.join("openapi/openapi.json"),
    ] {
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"{}").unwrap();
        assert!(
            fixture_control::verify_full_stack_runtime(&prepared.context.root, &prepared.output)
                .is_err()
        );
        fs::write(&path, original).unwrap();
        assert_eq!(
            fixture_control::verify_full_stack_runtime(&prepared.context.root, &prepared.output)
                .unwrap(),
            *receipt
        );
    }
    fs::write(
        Path::new(binaries["ryframe"].as_str().unwrap()),
        "tampered frozen API",
    )
    .unwrap();
    assert!(
        fixture_control::verify_full_stack_runtime(&prepared.context.root, &prepared.output)
            .is_err()
    );
    assert!(
        bridge::verify_build(
            &prepared.context.root,
            &prepared.output.join("backend-build.json"),
            backend
        )
        .is_err()
    );
}

#[test]
fn native_fixture_build_retains_partial_failure_and_rejects_source_or_configuration_drift() {
    for failure in [
        "command",
        "source",
        "configuration",
        "artifact",
        "publication",
        "contract",
    ] {
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
                    "publication" => fs::write(prepared.fixture.output.join("execution-source.json"), "{}")?,
                    "contract" => fs::write(prepared.fixture.frontend.join("openapi/openapi.json"), "{}")?,
                    "artifact" => return Ok(json!({"reason":"compiler-artifact","target":{"name":"ryframe-reset","kind":["lib"]},
                        "manifest_path":inputs.root.join("crates/ryframe/Cargo.toml"),"executable":prepared.context.root.join("README.md")}).to_string()),
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
    let cargo_home = if cfg!(windows) {
        "cargo_home"
    } else {
        "CARGO_HOME"
    };
    let rustup_home = if cfg!(windows) {
        "rustup_home"
    } else {
        "RUSTUP_HOME"
    };
    let inherited = [
        ("PATH", "system-path"),
        ("CARGO_TARGET_DIR", "D:/shared-target"),
        (cargo_home, "relative-cargo-home"),
        (rustup_home, "D:/synthetic-rustup-home"),
        ("CARGO_UNKNOWN_SETTING", "must-not-inherit"),
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
        ("CARGO_HOME", "relative-cargo-home"),
        ("RUSTUP_HOME", "D:/synthetic-rustup-home"),
        ("APP_ENV", "test"),
        ("TEMP", "bound-temp"),
        ("TMP", "bound-tmp"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    assert_eq!(actual, expected);
    let context = |values: &std::collections::BTreeMap<String, String>| {
        bridge::build_context::from_inputs(
            "cargo synthetic",
            "rustc synthetic\nhost: synthetic-target",
            values,
            1,
        )
        .unwrap()
    };
    let recorded = context(&actual);
    assert_eq!(recorded["jobs"], "1");
    for (key, replacement) in [
        ("CARGO_TARGET_DIR", "D:/other-target"),
        ("CARGO_BUILD_JOBS", "2"),
        ("CARGO_HOME", "other-relative-cargo-home"),
        ("RUSTUP_HOME", "D:/other-synthetic-rustup-home"),
    ] {
        assert!(
            recorded["environment"]["variables"]
                .as_array()
                .unwrap()
                .contains(&json!(key))
        );
        let mut changed = actual.clone();
        changed.insert(key.into(), replacement.into());
        let other = context(&changed);
        assert_eq!(other["jobs"], "1");
        assert_ne!(
            recorded["environment"]["sha256"],
            other["environment"]["sha256"]
        );
    }
    let absent = environment::configured(&private, []).unwrap();
    assert!(!absent.contains_key("CARGO_TARGET_DIR"));
    assert!(!absent.contains_key("CARGO_BUILD_JOBS"));
    assert!(!absent.contains_key("CARGO_HOME"));
    assert!(!absent.contains_key("RUSTUP_HOME"));
    let default = context(&absent);
    assert_eq!(default["jobs"], "1");
    for invalid in ["", "0", "000", "-1", "+1", "1.5", "one", " 1 "] {
        let inherited = [("CARGO_BUILD_JOBS".into(), invalid.into())];
        assert!(environment::configured(&private, inherited).is_err());
    }
}
