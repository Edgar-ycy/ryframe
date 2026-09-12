use super::cli::{
    CheckCommand, CliError, Command, ExistingReferenceSide, FixtureExpectedSources,
    FixturePrepareCommand, FixturePrepareOptions, FreshTargetCommand, FreshTargetOperation,
    FreshTargetOptions, FullStackCommand, RecoveryCommand, RecoveryInputsCommand,
    RecoveryReferenceCommand, ReferencePlanAction, RestoreInputSide, parse,
};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn parse_command(values: &[&str]) -> std::result::Result<Command, CliError> {
    parse(strings(values)).map(|cli| cli.command)
}

fn parse_recovery_owned(values: Vec<String>) -> std::result::Result<RecoveryCommand, CliError> {
    let command = strings(&["check", "recovery"])
        .into_iter()
        .chain(values)
        .collect();
    match parse(command)?.command {
        Command::Check(CheckCommand::Recovery(command)) => Ok(command),
        _ => unreachable!("测试只构造 recovery 命令"),
    }
}

struct RecoveryArgumentFixture {
    directory: std::path::PathBuf,
}

impl RecoveryArgumentFixture {
    fn new(label: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = super::workspace::root_dir().join(format!(
            ".local-tests/recovery-args-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }

    fn file(&self, name: &str) -> String {
        let path = self.directory.join(name);
        std::fs::write(&path, "{}").unwrap();
        path.to_string_lossy().into_owned()
    }

    fn new_path(&self, name: &str) -> String {
        self.directory.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for RecoveryArgumentFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
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
fn reference_plan_and_read_only_commands_are_typed() {
    let fixture = RecoveryArgumentFixture::new("reference");
    let plan = fixture.file("reference plan.json");
    let summary = parse_recovery_owned(strings(&["plan", "--plan", &plan])).unwrap();
    assert!(matches!(
        summary,
        RecoveryCommand::Reference(RecoveryReferenceCommand::Plan(options))
            if matches!(options.action, ReferencePlanAction::Summary)
    ));

    let existing = parse_recovery_owned(strings(&["check-existing", "--plan", &plan])).unwrap();
    assert!(matches!(
        existing,
        RecoveryCommand::Reference(RecoveryReferenceCommand::CheckExisting {
            side: ExistingReferenceSide::Target,
            ..
        })
    ));

    let backup = fixture.file("backup.json");
    let comparison = fixture.file("comparison.json");
    let arm = fixture.file("arm.json");
    let fresh = fixture.file("fresh.json");
    let product = fixture.file("product.json");
    let output = fixture.new_path("target plan.json");
    let published = parse_recovery_owned(strings(&[
        "plan",
        "--plan",
        &plan,
        "--backup-receipt",
        &backup,
        "--comparison-sources",
        &comparison,
        "--arm-input",
        &arm,
        "--fresh-target-verify",
        &fresh,
        "--product-plan",
        &product,
        "--output",
        &output,
        "--write",
    ]))
    .unwrap();
    assert!(matches!(
        published,
        RecoveryCommand::Reference(RecoveryReferenceCommand::Plan(options))
            if matches!(options.action, ReferencePlanAction::Publish { .. })
    ));
}

#[test]
fn reference_writing_commands_require_explicit_authorization() {
    let fixture = RecoveryArgumentFixture::new("reference-write");
    let plan = fixture.file("reference plan.json");
    for (operation, extra) in [
        ("dataset", strings(&["--write"])),
        (
            "backup",
            strings(&[
                "--inventory",
                &fixture.file("inventory.json"),
                "--source-generation",
                &fixture.file("generation.json"),
                "--source-export-result",
                &fixture.file("export.json"),
                "--write",
            ]),
        ),
        (
            "restore",
            strings(&[
                "--backup-root",
                fixture.directory.to_str().unwrap(),
                "--record",
                &fixture.file("record.json"),
                "--target-plan",
                &fixture.file("target.json"),
                "--runtime-registration",
                &fixture.file("runtime.json"),
                "--write",
            ]),
        ),
        (
            "copy",
            strings(&[
                "--backup-root",
                fixture.directory.to_str().unwrap(),
                "--copy-id",
                "damaged-copy",
                "--write",
            ]),
        ),
        (
            "damage",
            strings(&[
                "--backup-root",
                fixture.directory.to_str().unwrap(),
                "--artifact",
                "databases/control.sql",
                "--missing",
                "--write",
            ]),
        ),
    ] {
        let command = strings(&[operation, "--plan", &plan])
            .into_iter()
            .chain(extra)
            .collect();
        let parsed = parse_recovery_owned(command).unwrap();
        assert!(matches!(parsed, RecoveryCommand::Reference(command) if command.writes()));
    }
}

#[test]
fn reference_parser_rejects_ambiguous_or_unsafe_arguments_before_execution() {
    let fixture = RecoveryArgumentFixture::new("reference-invalid");
    let plan = fixture.file("plan.json");
    let invalid = [
        strings(&["dataset", "--plan", &plan]),
        strings(&["check-existing", "--plan", &plan, "--write"]),
        strings(&["plan", "--plan", &plan, "--arm-input", &plan]),
        strings(&["plan", "--plan", &plan, "--target-plan", &plan, "--write"]),
        strings(&[
            "copy",
            "--plan",
            &plan,
            "--backup-root",
            fixture.directory.to_str().unwrap(),
            "--copy-id",
            "Uppercase",
            "--write",
        ]),
        strings(&[
            "damage",
            "--plan",
            &plan,
            "--backup-root",
            fixture.directory.to_str().unwrap(),
            "--artifact",
            "../control.sql",
            "--write",
        ]),
        strings(&["plan", "--plan", &plan, "--plan", &plan]),
        strings(&["plan", "--plan", ".local-tests/../outside.json"]),
        strings(&["plan", "--plan", "outside.json"]),
        strings(&["plan", "--plan"]),
        strings(&["plan", "--unknown", "value", "--plan", &plan]),
    ];
    for arguments in invalid {
        assert!(
            parse_recovery_owned(arguments.clone()).is_err(),
            "{arguments:?}"
        );
    }
}

#[test]
fn restore_inputs_commands_are_typed_and_require_explicit_publication() {
    let fixture = RecoveryArgumentFixture::new("inputs");
    let arm = fixture.file("arm.json");
    let fresh = fixture.file("fresh.json");
    let work = fixture.new_path("work");
    let reference = parse_recovery_owned(strings(&[
        "inputs",
        "reference",
        "--arm-input",
        &arm,
        "--fresh-target-verify",
        &fresh,
        "--side",
        "base",
        "--id",
        "reference-r1",
        "--work-dir",
        &work,
    ]))
    .unwrap();
    assert!(matches!(
        reference,
        RecoveryCommand::Inputs(RecoveryInputsCommand::Reference(options))
            if options.side == RestoreInputSide::Base && options.publication.is_none()
    ));

    let reference_plan = fixture.file("reference.json");
    let backup = fixture.file("backup.json");
    let comparison = fixture.file("comparison.json");
    let output = fixture.new_path("product output.json");
    let product = parse_recovery_owned(strings(&[
        "inputs",
        "product",
        "--reference-plan",
        &reference_plan,
        "--backup-receipt",
        &backup,
        "--comparison-sources",
        &comparison,
        "--arm-input",
        &arm,
        "--fresh-target-verify",
        &fresh,
        "--side",
        "candidate",
        "--id",
        "restore-r1",
        "--fault-at",
        "2026-09-12T00:00:00.000Z",
        "--output",
        &output,
        "--write",
    ]))
    .unwrap();
    assert!(matches!(
        product,
        RecoveryCommand::Inputs(RecoveryInputsCommand::Product(options))
            if options.side == RestoreInputSide::Candidate && options.publication.is_some()
    ));

    let target = fixture.file("target.json");
    let record = fixture.file("record.json");
    let bindings = parse_recovery_owned(strings(&[
        "inputs",
        "bindings",
        "--reference-plan",
        &reference_plan,
        "--target-plan",
        &target,
        "--backup-receipt",
        &backup,
        "--record",
        &record,
    ]))
    .unwrap();
    assert!(matches!(
        bindings,
        RecoveryCommand::Inputs(RecoveryInputsCommand::Bindings(options))
            if options.publication.is_none()
    ));
}

#[test]
fn restore_inputs_parser_rejects_partial_cross_stage_and_unsafe_values() {
    let fixture = RecoveryArgumentFixture::new("inputs-invalid");
    let file = fixture.file("input.json");
    let invalid = [
        strings(&["inputs", "reference", "--side", "base"]),
        strings(&[
            "inputs",
            "reference",
            "--arm-input",
            &file,
            "--fresh-target-verify",
            &file,
            "--side",
            "other",
            "--id",
            "r1",
            "--work-dir",
            &fixture.new_path("work"),
        ]),
        strings(&[
            "inputs",
            "reference",
            "--arm-input",
            &file,
            "--fresh-target-verify",
            &file,
            "--side",
            "base",
            "--id",
            "r1",
            "--work-dir",
            &fixture.new_path("work"),
            "--write",
        ]),
        strings(&[
            "inputs",
            "product",
            "--reference-plan",
            &file,
            "--backup-receipt",
            &file,
            "--comparison-sources",
            &file,
            "--arm-input",
            &file,
            "--fresh-target-verify",
            &file,
            "--side",
            "base",
            "--id",
            "r1",
            "--fault-at",
            "2026-09-12T00:00:00+08:00",
        ]),
        strings(&[
            "inputs",
            "bindings",
            "--reference-plan",
            &file,
            "--target-plan",
            &file,
            "--backup-receipt",
            &file,
            "--record",
            &file,
            "--id",
            "other",
        ]),
        strings(&[
            "inputs",
            "bindings",
            "--reference-plan",
            &file,
            "--target-plan",
            &file,
            "--backup-receipt",
            &file,
            "--record",
            &file,
            "--record",
            &file,
        ]),
        strings(&[
            "inputs",
            "bindings",
            "--reference-plan",
            ".local-tests/../outside.json",
            "--target-plan",
            &file,
            "--backup-receipt",
            &file,
            "--record",
            &file,
        ]),
    ];
    assert_invalid_recovery_arguments(invalid);
}

fn assert_invalid_recovery_arguments<const N: usize>(invalid: [Vec<String>; N]) {
    for arguments in invalid {
        assert!(
            parse_recovery_owned(arguments.clone()).is_err(),
            "{arguments:?}"
        );
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
    let fixture = RecoveryArgumentFixture::new("fixture-generation");
    let output = fixture.new_path("device");
    assert_eq!(
        parse_recovery_owned(vec![
            "fixture".to_owned(),
            "--output-dir".to_owned(),
            output.clone(),
            "--expected-backend-sha".to_owned(),
            "a123456789012345678901234567890123456789".to_owned(),
            "--expected-frontend-sha".to_owned(),
            "b123456789012345678901234567890123456789".to_owned(),
            "--write".to_owned(),
        ])
        .unwrap(),
        RecoveryCommand::FixturePrepare(FixturePrepareCommand::Run(FixturePrepareOptions {
            output_dir: output.into(),
            expected_sources: Some(FixtureExpectedSources {
                backend_sha: "a123456789012345678901234567890123456789".to_owned(),
                frontend_sha: "b123456789012345678901234567890123456789".to_owned(),
            }),
        }))
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
