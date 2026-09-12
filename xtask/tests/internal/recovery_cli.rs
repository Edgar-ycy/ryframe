use super::cli::{
    CheckCommand, CliError, Command, FreshTargetCommand, FreshTargetOperation, FreshTargetOptions,
    FullStackCommand, RecoveryCommand, parse,
};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn parse_command(values: &[&str]) -> std::result::Result<Command, CliError> {
    parse(strings(values)).map(|cli| cli.command)
}
#[test]
fn parses_recovery_check_groups() {
    assert_eq!(
        parse_command(&[
            "check",
            "recovery",
            "runtime",
            "verify",
            "--receipt",
            "runtime.json",
        ])
        .unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::Runtime(strings(
            &["verify", "--receipt", "runtime.json",]
        ))))
    );
    for operation in [
        "build", "register", "start", "status", "stop", "recover", "bind",
    ] {
        assert!(matches!(
            parse_command(&["check", "recovery", "runtime", operation]),
            Ok(Command::Check(CheckCommand::Recovery(RecoveryCommand::Runtime(arguments))))
                if arguments == strings(&[operation])
        ));
    }
    assert!(parse_command(&["check", "recovery", "runtime", "restart"]).is_err());
    assert!(parse_command(&["check", "recovery", "source", "quiesce"]).is_err());
    let workspace = super::workspace::root_dir().join(".local-tests/隔离 target");
    assert_eq!(
        parse_command(&[
            "check",
            "recovery",
            "fresh-target",
            "--workspace",
            ".local-tests/隔离 target",
            "--operation",
            "status",
        ])
        .unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::FreshTarget(
            FreshTargetCommand::Run(FreshTargetOptions {
                operation: FreshTargetOperation::Status,
                workspace,
                request: None,
                environment: None,
                storage_run: None,
                observation_dir: None,
                write: false,
            })
        )))
    );
    assert_eq!(
        parse_command(&[
            "check",
            "recovery",
            "source",
            "comparison-verify",
            "--receipt",
            "comparison.json",
        ])
        .unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::Source(strings(&[
            "comparison-verify",
            "--receipt",
            "comparison.json"
        ]))))
    );
    for values in [
        ["check", "recovery", "source"].as_slice(),
        ["check", "recovery", "inputs"].as_slice(),
        ["check", "recovery", "inputs", "unknown"].as_slice(),
        ["check", "recovery", "source", "unknown"].as_slice(),
        ["check", "recovery", "runtime", "unknown"].as_slice(),
        ["check", "recovery", "missing"].as_slice(),
    ] {
        assert!(parse_command(values).is_err());
    }
}

#[test]
fn fresh_target_owns_arguments_and_write_policy_in_the_rust_parser() {
    let valid = [
        "check",
        "recovery",
        "fresh-target",
        "--workspace",
        ".local-tests/fresh target",
        "--operation",
        "prepare",
        "--request",
        ".local-tests/request.json",
        "--environment",
        ".local-tests/environment.json",
        "--storage-run",
        ".local-tests/storage run",
        "--write",
    ];
    let parsed = parse_command(&valid).unwrap();
    let Command::Check(CheckCommand::Recovery(RecoveryCommand::FreshTarget(
        FreshTargetCommand::Run(options),
    ))) = parsed
    else {
        panic!("fresh-target 应解析为结构化请求");
    };
    assert_eq!(options.operation, FreshTargetOperation::Prepare);
    assert!(options.write);
    assert!(options.workspace.is_absolute());
    assert!(options.request.unwrap().is_absolute());

    for invalid in [
        vec![
            "--workspace",
            ".local-tests/a",
            "--operation",
            "status",
            "--write",
        ],
        vec![
            "--workspace",
            ".local-tests/a",
            "--operation",
            "prepare",
            "--write",
        ],
        vec![
            "--workspace",
            ".local-tests/a",
            "--operation",
            "verify",
            "--write",
        ],
        vec!["--workspace", ".local-tests/a", "--operation", "initialize"],
        vec!["--workspace", ".local-tests/a", "--operation", "unknown"],
        vec![
            "--workspace",
            ".local-tests/a",
            "--operation",
            "status",
            "--unknown",
        ],
        vec![
            "--workspace",
            ".local-tests/a",
            "--workspace",
            ".local-tests/b",
            "--operation",
            "status",
        ],
        vec!["--workspace", ".local-tests/a", "--operation"],
        vec!["--workspace", "outside", "--operation", "status"],
        vec![
            "--workspace",
            ".local-tests/../outside",
            "--operation",
            "status",
        ],
    ] {
        let mut command = strings(&["check", "recovery", "fresh-target"]);
        command.extend(invalid.into_iter().map(ToOwned::to_owned));
        assert!(parse(command).is_err());
    }
}

#[test]
fn parses_fixture_generation_and_rejects_ambiguous_sources() {
    assert_eq!(
        parse_command(&[
            "check",
            "recovery",
            "fixture",
            "--output-dir",
            "fixture",
            "--expected-backend-sha",
            "a123456789012345678901234567890123456789",
            "--expected-frontend-sha",
            "b123456789012345678901234567890123456789",
            "--write",
        ])
        .unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::Fixture(strings(
            &[
                "--output-dir",
                "fixture",
                "--expected-backend-sha",
                "a123456789012345678901234567890123456789",
                "--expected-frontend-sha",
                "b123456789012345678901234567890123456789",
                "--write",
            ]
        ))))
    );
    for invalid in [
        vec!["--output-dir", "fixture"],
        vec!["--output-dir", "fixture", "--write", "--write"],
        vec!["--output-dir", "fixture", "--unknown", "value", "--write"],
        vec![
            "--output-dir",
            "fixture",
            "--expected-backend-sha",
            "a123456789012345678901234567890123456789",
            "--write",
        ],
        vec![
            "--output-dir",
            "fixture",
            "--expected-backend-sha",
            "main",
            "--expected-frontend-sha",
            "b123456789012345678901234567890123456789",
            "--write",
        ],
    ] {
        let mut command = vec!["check", "recovery", "fixture"];
        command.extend(invalid);
        assert!(parse_command(&command).is_err(), "{command:?}");
    }
    for operation in ["artifact", "retention"] {
        assert_eq!(
            parse_command(&[
                "check",
                "recovery",
                "fixture",
                operation,
                "inspect",
                "--runtime-dir",
                "D:/验收/runtime",
            ])
            .unwrap(),
            Command::Check(CheckCommand::Recovery(RecoveryCommand::Fixture(strings(
                &[operation, "inspect", "--runtime-dir", "D:/验收/runtime",]
            ))))
        );
    }
}

#[test]
fn parses_typed_full_stack_operations_and_rejects_ambiguous_arguments() {
    for (operation, expected) in [
        ("prepare", FullStackCommand::Prepare),
        ("start", FullStackCommand::Start),
        ("collect", FullStackCommand::Collect),
    ] {
        assert_eq!(
            parse_command(&["check", "recovery", "full-stack", operation]).unwrap(),
            Command::Check(CheckCommand::Recovery(RecoveryCommand::FullStack(expected)))
        );
    }
    #[cfg(windows)]
    let environment_file = "D:/运行 目录/github-environment";
    #[cfg(not(windows))]
    let environment_file = "/tmp/运行 目录/github-environment";
    assert_eq!(
        parse_command(&[
            "check",
            "recovery",
            "full-stack",
            "rate-limit",
            "--environment-file",
            environment_file,
        ])
        .unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::FullStack(
            FullStackCommand::RateLimit {
                environment_file: environment_file.into(),
            }
        )))
    );
    for values in [
        ["check", "recovery", "full-stack"].as_slice(),
        ["check", "recovery", "full-stack", "unknown"].as_slice(),
        ["check", "recovery", "full-stack", "prepare", "extra"].as_slice(),
        ["check", "recovery", "full-stack", "rate-limit"].as_slice(),
        [
            "check",
            "recovery",
            "full-stack",
            "rate-limit",
            "--environment-file",
            "relative.env",
        ]
        .as_slice(),
        [
            "check",
            "recovery",
            "full-stack",
            "rate-limit",
            "--environment-file",
            environment_file,
            "--environment-file",
            environment_file,
        ]
        .as_slice(),
    ] {
        assert!(parse_command(values).is_err(), "{values:?}");
    }
}

fn monitoring_bind_arguments() -> Vec<String> {
    strings(&[
        "bind",
        "--runtime-receipt",
        "D:/恢复/runtime.json",
        "--target-plan",
        "D:/恢复/target.json",
        "--output",
        "D:/恢复/监控/binding.json",
        "--run-id",
        "monitor-r1",
        "--metrics-token-file",
        "D:/恢复/监控/metrics-token.txt",
        "--prometheus",
        "D:/tools/prometheus.exe",
        "--promtool",
        "D:/tools/promtool.exe",
        "--alertmanager",
        "D:/tools/alertmanager.exe",
        "--amtool",
        "D:/tools/amtool.exe",
        "--prometheus-port",
        "29090",
        "--alertmanager-port",
        "29093",
        "--webhook-port",
        "29094",
        "--write",
    ])
}

fn parse_monitoring(arguments: Vec<String>) -> std::result::Result<Command, CliError> {
    parse(
        strings(&["check", "recovery", "monitoring"])
            .into_iter()
            .chain(arguments)
            .collect(),
    )
    .map(|cli| cli.command)
}

#[test]
fn parses_monitoring_read_and_write_operations() {
    assert_eq!(
        parse_command(&[
            "check",
            "recovery",
            "monitoring",
            "status",
            "--binding",
            "D:/隔离 监控/binding.json",
        ])
        .unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::Monitoring(
            strings(&["status", "--binding", "D:/隔离 监控/binding.json"])
        )))
    );
    let arguments = monitoring_bind_arguments();
    assert_eq!(
        parse_monitoring(arguments.clone()).unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::Monitoring(
            arguments
        )))
    );
}

#[test]
fn rejects_invalid_monitoring_operations_before_execution() {
    let mut duplicate_ports = monitoring_bind_arguments();
    let alertmanager_port = duplicate_ports
        .iter()
        .position(|value| value == "--alertmanager-port")
        .unwrap()
        + 1;
    duplicate_ports[alertmanager_port] = "29090".into();
    let mut privileged_port = monitoring_bind_arguments();
    let prometheus_port = privileged_port
        .iter()
        .position(|value| value == "--prometheus-port")
        .unwrap()
        + 1;
    privileged_port[prometheus_port] = "80".into();
    for values in [
        Vec::new(),
        strings(&["unknown"]),
        strings(&["start", "--binding", "one"]),
        strings(&["status", "--binding", "one", "--write"]),
        strings(&["status", "--binding", "one", "--binding", "two"]),
        strings(&["status", "--binding", "one", "--unknown"]),
        strings(&["bind", "--write"]),
        duplicate_ports,
        privileged_port,
    ] {
        assert!(parse_monitoring(values.clone()).is_err(), "{values:?}");
    }
}

#[test]
fn parses_recovery_dataset_and_clone_groups() {
    assert!(
        parse_command(&[
            "check",
            "recovery",
            "dataset-prepare",
            "--plan",
            "reference.json",
        ])
        .is_err()
    );
    assert_eq!(
        parse_command(&["check", "recovery", "clone", "status", "--run-dir", "run"]).unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::Clone(strings(&[
            "status",
            "--run-dir",
            "run",
        ]))))
    );
    assert_eq!(
        parse_command(&[
            "check",
            "recovery",
            "clone",
            "maintenance",
            "--operation",
            "verify",
            "--output",
            "build.json",
        ])
        .unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::Clone(strings(&[
            "maintenance",
            "--operation",
            "verify",
            "--output",
            "build.json",
        ]))))
    );
}
