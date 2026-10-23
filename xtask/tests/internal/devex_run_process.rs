use super::*;

#[test]
fn native_step_keeps_only_app_env_from_project_protocol_environment() {
    let definition = super::super::super::model::DevexSuite::RuntimeApi
        .definition("10")
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let roots = StepRoots {
        backend: temp.path(),
        frontend: temp.path(),
        runner_frontend: temp.path(),
        target: temp.path(),
    };
    let environment = project_protocol_environment();
    let input = super::super::super::runtime_native::Input {
        backend: temp.path().into(),
        frontend: temp.path().into(),
        runner_frontend: temp.path().into(),
        driver: temp.path().into(),
        output: temp.path().join("unused.json"),
        suite: "api".into(),
        cache: "warm".into(),
        concurrency: 10,
        input_sha256: "a".repeat(64),
        driver_fingerprint: format!("sha256:{}", "d".repeat(64)),
        runner_frontend_fingerprint: format!("sha256:{}", "e".repeat(64)),
        source_fingerprints: super::super::super::metadata::SourceFingerprints {
            backend: format!("sha256:{}", "b".repeat(64)),
            frontend: Some(format!("sha256:{}", "f".repeat(64))),
            runner_frontend: Some(format!("sha256:{}", "e".repeat(64))),
        },
        native_tool: super::super::super::job_evidence::artifact::current_binding().unwrap(),
        app_env: "test".into(),
    };
    let command = step_command(
        &definition.steps[0],
        roots,
        definition,
        &environment,
        &StepInvocation {
            variant: "10",
            label: "test",
            runtime_input: Some(&input),
        },
    )
    .unwrap();
    let clean_command = step_command(
        &definition.steps[0],
        roots,
        definition,
        &BTreeMap::from([("CARGO_BUILD_JOBS".into(), "1".into())]),
        &StepInvocation {
            variant: "10",
            label: "test",
            runtime_input: Some(&input),
        },
    )
    .unwrap();
    let active_environment = |command: &Command| {
        command
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value.to_owned())))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(
        active_environment(&command),
        active_environment(&clean_command)
    );
    for (key, value) in command.get_envs() {
        let name = key.to_string_lossy();
        if super::super::super::runtime_native::entry::private_environment(&name) {
            assert!(value.is_none(), "{name}");
        }
    }
    assert!(
        command
            .get_envs()
            .any(|(key, value)| key == "APP_ENV" && value == Some(std::ffi::OsStr::new("test")))
    );
    assert!(
        command
            .get_envs()
            .any(|(key, value)| key == "CARGO_BUILD_JOBS"
                && value == Some(std::ffi::OsStr::new("1")))
    );
    assert!(
        command
            .get_envs()
            .all(|(_, value)| value != Some(std::ffi::OsStr::new("must-not-leak")))
    );
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![std::ffi::OsStr::new("__devex-runtime")]
    );
}

#[test]
fn save_step_passes_validated_case_and_output_as_arguments() {
    let definition = super::super::super::model::DevexSuite::CargoDevSave
        .definition("worker-only")
        .unwrap();
    let roots = StepRoots {
        backend: Path::new("D:/measured-source"),
        frontend: Path::new("D:/frontend"),
        runner_frontend: Path::new("D:/runner-frontend"),
        target: Path::new("D:/sample-target"),
    };
    let invocation = StepInvocation {
        variant: "worker-only",
        label: "sample-001",
        runtime_input: None,
    };
    let command = step_command(
        &definition.steps[0],
        roots,
        definition,
        &BTreeMap::new(),
        &invocation,
    )
    .unwrap();
    let args = command
        .get_args()
        .map(|arg| arg.to_str().unwrap())
        .collect::<Vec<_>>();
    assert_ne!(roots.backend, roots.target);
    assert_eq!(command.get_current_dir(), Some(roots.backend));
    assert_eq!(
        args,
        [
            "xtask",
            "dev",
            "--measure-once",
            "--workspace-root",
            "D:/measured-source",
            "--case",
            "worker-only",
            "--output",
            "D:/sample-target/cargo-dev-save-result.json",
        ]
    );
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == "CARGO_TARGET_DIR")
            .and_then(|(_, value)| value),
        Some(roots.target.as_os_str()),
    );
    assert!(command.get_envs().all(
        |(key, _)| key != "RYFRAME_DEVEX_SAVE_CASE" && key != "RYFRAME_DEVEX_SAVE_RESULT_PATH"
    ));
    let invalid = StepInvocation {
        variant: "unregistered",
        label: "sample-001",
        runtime_input: None,
    };
    assert!(
        step_command(
            &definition.steps[0],
            roots,
            definition,
            &BTreeMap::new(),
            &invalid
        )
        .is_err()
    );
}

fn project_protocol_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("APP_ENV".into(), "poison".into()),
        ("APP_DATABASE_PASSWORD".into(), "must-not-leak".into()),
        ("VITE_API_URL".into(), "must-not-leak".into()),
        ("SNOWFLAKE_WORKER_ID".into(), "must-not-leak".into()),
        ("RYFRAME_DEVEX_CONCURRENCY".into(), "poison".into()),
        ("RYFRAME_DEVEX_CACHE".into(), "warm".into()),
        ("RYFRAME_DEVEX_RUNTIME_INPUT_SHA256".into(), "a".repeat(64)),
        (
            "RYFRAME_DEVEX_DRIVER_FINGERPRINT".into(),
            format!("sha256:{}", "d".repeat(64)),
        ),
        (
            "RYFRAME_DEVEX_RUNNER_FRONTEND_FINGERPRINT".into(),
            format!("sha256:{}", "e".repeat(64)),
        ),
        ("RYFRAME_DEVEX_SOURCE_FINGERPRINTS".into(), "{}".into()),
        ("CARGO_BUILD_JOBS".into(), "1".into()),
    ])
}

#[test]
fn retired_roots_do_not_reach_real_step_children() {
    let mut command = environment_probe("retired_roots_step_driver");
    command
        .env("RYFRAME_DEVEX_TARGET_ROOT", "ignored-inherited-target")
        .env("RYFRAME_FAST_CHECK_CACHE_ROOT", "ignored-inherited-cache")
        .env("CARGO_BUILD_JOBS", "7")
        .env("SCCACHE_CACHE_SIZE", "1G")
        .env("CI", "true");
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
#[ignore = "仅由受控子进程测试调用，验证继承环境"]
fn retired_roots_step_driver() {
    use super::super::super::model::DevexSuite;
    assert_eq!(
        std::env::var("RYFRAME_DEVEX_TARGET_ROOT").unwrap(),
        "ignored-inherited-target"
    );
    assert_eq!(
        std::env::var("RYFRAME_FAST_CHECK_CACHE_ROOT").unwrap(),
        "ignored-inherited-cache"
    );
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("explicit-target");
    let roots = StepRoots {
        backend: temp.path(),
        frontend: temp.path(),
        runner_frontend: temp.path(),
        target: &target,
    };
    let invocation = StepInvocation {
        variant: "default",
        label: "sample-001",
        runtime_input: None,
    };
    let environment = BTreeMap::from([
        (
            "RYFRAME_DEVEX_TARGET_ROOT".into(),
            "ignored-explicit-target".into(),
        ),
        (
            "RYFRAME_FAST_CHECK_CACHE_ROOT".into(),
            "ignored-explicit-cache".into(),
        ),
    ]);
    let executable = std::env::current_exe().unwrap();
    let probe = StepDefinition {
        working_directory: WorkingDirectory::Backend,
        program: Box::leak(executable.to_str().unwrap().to_owned().into_boxed_str()),
        args: &[],
    };
    for (suite, variant) in [
        (DevexSuite::ResourceGate, "auto"),
        (DevexSuite::RustGate, "default"),
        (DevexSuite::FrontendFast, "default"),
    ] {
        let definition = suite.definition(variant).unwrap();
        let invocation = StepInvocation {
            variant,
            ..invocation
        };
        let actual = step_command(
            &definition.steps[0],
            roots,
            definition,
            &environment,
            &invocation,
        )
        .unwrap();
        let clean = step_command(
            &definition.steps[0],
            roots,
            definition,
            &BTreeMap::new(),
            &invocation,
        )
        .unwrap();
        assert_eq!(
            actual.get_args().collect::<Vec<_>>(),
            clean.get_args().collect::<Vec<_>>()
        );
        assert_eq!(actual.get_current_dir(), clean.get_current_dir());
        assert_eq!(
            actual.get_envs().collect::<Vec<_>>(),
            clean.get_envs().collect::<Vec<_>>()
        );
        if definition.steps[0].program == "cargo" {
            assert!(
                actual
                    .get_envs()
                    .any(|(k, v)| k == "CARGO_TARGET_DIR" && v == Some(target.as_os_str()))
            );
        }
        let mut child = step_command(&probe, roots, definition, &environment, &invocation).unwrap();
        child
            .args(environment_probe_arguments("retired_roots_step_observer"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{suite:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}

#[test]
#[ignore = "仅由受控步骤子进程调用，验证实际进程环境"]
fn retired_roots_step_observer() {
    assert!(std::env::var_os("RYFRAME_DEVEX_TARGET_ROOT").is_none());
    assert!(std::env::var_os("RYFRAME_FAST_CHECK_CACHE_ROOT").is_none());
    assert_eq!(std::env::var("CARGO_BUILD_JOBS").unwrap(), "7");
    assert_eq!(std::env::var("SCCACHE_CACHE_SIZE").unwrap(), "1G");
    assert_eq!(std::env::var("CI").unwrap(), "true");
}

fn environment_probe(name: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(environment_probe_arguments(name));
    command
}

fn environment_probe_arguments(name: &str) -> Vec<String> {
    let (_, module) = module_path!().split_once("::").unwrap();
    vec![
        "--exact".into(),
        format!("{module}::{name}"),
        "--ignored".into(),
        "--nocapture".into(),
        "--test-threads=1".into(),
    ]
}
