use super::*;

#[test]
fn common_save_binds_each_arm_to_its_protocol_and_preserves_sample_results() {
    use crate::devex::{BaselineContract, DevexSuite, PairedArm};

    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("warm");
    std::fs::create_dir(&target).unwrap();
    let roots = StepRoots {
        backend: root.path(),
        frontend: root.path(),
        runner_frontend: root.path(),
        target: &target,
    };
    let invocation = StepInvocation {
        variant: "cancellation",
        label: "sample-002",
        runtime_input: None,
        pairing: None,
    };
    let contract = Some(BaselineContract::StableReadinessB0CommonToolchainV1);
    let expected = target.join("cargo-dev-save-result-sample-002.json");
    for arm in [PairedArm::Baseline, PairedArm::Candidate] {
        let definition = DevexSuite::CargoDevSave
            .paired_definition(invocation.variant, contract, arm)
            .unwrap();
        let command = step_command(
            &definition.steps[0],
            roots,
            definition,
            &BTreeMap::new(),
            &invocation,
        )
        .unwrap();
        assert_eq!(command.get_current_dir(), Some(root.path()));
        let args: Vec<_> = command.get_args().collect();
        let environment = |name: &str| {
            command
                .get_envs()
                .find_map(|(key, value)| (key == name).then_some(value))
        };
        if arm == PairedArm::Baseline {
            assert_eq!(args, ["xtask", "dev", "--measure-once"]);
            assert_eq!(
                environment("RYFRAME_DEVEX_SAVE_CASE"),
                Some(Some(std::ffi::OsStr::new("cancellation")))
            );
            assert_eq!(
                environment("RYFRAME_DEVEX_SAVE_RESULT_PATH"),
                Some(Some(expected.as_os_str()))
            );
            assert_eq!(environment("RYFRAME_WORKSPACE_ROOT"), Some(None));
        } else {
            assert_eq!(args.last().copied(), Some(expected.as_os_str()));
            assert_eq!(environment("RYFRAME_DEVEX_SAVE_CASE"), Some(None));
            assert_eq!(environment("RYFRAME_DEVEX_SAVE_RESULT_PATH"), Some(None));
        }
        assert_eq!(environment("RYFRAME_DEVEX_TARGET_ROOT"), Some(None));
        let result =
            super::super::save::result_path(&target, invocation.label, definition).unwrap();
        assert_eq!(result, expected);
        std::fs::write(&result, b"prior receipt").unwrap();
        assert!(super::super::save::result_path(&target, invocation.label, definition).is_err());
        assert_eq!(std::fs::read(&result).unwrap(), b"prior receipt");
        std::fs::remove_file(&result).unwrap();
    }
}

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
            pairing: None,
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
            pairing: None,
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
fn paired_save_command_passes_private_directory_to_both_protocols() {
    use super::super::super::model::{BaselineContract, DevexSuite, PairedArm, SaveConfigBinding};

    let path = if cfg!(windows) {
        r"\\?\D:\private config"
    } else {
        "/private config"
    };
    let expected = if cfg!(windows) {
        r"D:\private config"
    } else {
        path
    };
    let contract = Some(BaselineContract::StableReadinessB0CommonToolchainV1);
    let roots = StepRoots {
        backend: Path::new("D:/measured-source"),
        frontend: Path::new("D:/frontend"),
        runner_frontend: Path::new("D:/runner-frontend"),
        target: Path::new("D:/sample-target"),
    };
    for arm in [PairedArm::Baseline, PairedArm::Candidate] {
        let pairing = PairingMetadata {
            comparison_id: "save-pair".into(),
            arm,
            baseline_contract: contract,
            baseline_provenance: None,
            workload_contract: None,
            save_config: Some(SaveConfigBinding {
                directory: path.into(),
                files: BTreeMap::new(),
            }),
        };
        let definition = DevexSuite::CargoDevSave
            .paired_definition("config-only", contract, arm)
            .unwrap();
        let command = step_command(
            &definition.steps[0],
            roots,
            definition,
            &BTreeMap::new(),
            &StepInvocation {
                variant: "config-only",
                label: "sample-001",
                runtime_input: None,
                pairing: Some(&pairing),
            },
        )
        .unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(args[args.len() - 2..], ["--config-dir", expected]);
        assert_eq!(args.iter().filter(|arg| **arg == "--config-dir").count(), 1);
        assert!(
            command
                .get_envs()
                .all(|(_, value)| value != Some(std::ffi::OsStr::new(expected)))
        );
    }
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
        pairing: None,
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
    assert!(command.get_envs().all(|(key, value)| {
        (key != "RYFRAME_DEVEX_SAVE_CASE" && key != "RYFRAME_DEVEX_SAVE_RESULT_PATH")
            || value.is_none()
    }));
    let invalid = StepInvocation {
        variant: "unregistered",
        label: "sample-001",
        runtime_input: None,
        pairing: None,
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
