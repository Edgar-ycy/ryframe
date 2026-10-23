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
    let environment = BTreeMap::from([
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
    ]);
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
