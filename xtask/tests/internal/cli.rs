use std::path::PathBuf;

use super::cli::{
    ApiGenerateCommand, BuildOptions, BuildProfile, CheckCommand, CheckOptions, CheckScope,
    CiCommand, CliError, Command, DataCommand, GenerateCommand, MigrationCommand,
    MigrationOperation, MigrationTarget, RecoveryCommand, ResourceAction, ResourceCommand,
    ResourceTarget, parse,
};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn parse_command(values: &[&str]) -> std::result::Result<Command, CliError> {
    parse(strings(values)).map(|cli| cli.command)
}

#[test]
fn exposes_exactly_five_top_level_command_families() {
    assert!(matches!(parse_command(&["dev"]), Ok(Command::Dev { .. })));
    assert!(matches!(parse_command(&["check"]), Ok(Command::Check(_))));
    assert_eq!(
        parse_command(&["build"]).unwrap(),
        Command::Build(BuildOptions {
            profile: BuildProfile::Release,
            real: false,
            plan: false,
        })
    );
    assert!(parse_command(&["check", "recovery", "fixture", "status",]).is_err());
    assert_eq!(
        parse_command(&["generate"]).unwrap(),
        Command::Generate(GenerateCommand::Help)
    );
    assert_eq!(
        parse_command(&["data"]).unwrap(),
        Command::Data(DataCommand::Help)
    );
    for removed in [
        "doctor",
        "verify",
        "resource",
        "api-sync",
        "migrate",
        "ci",
        "devex",
        "contract",
        "feature-matrix",
        "release-verify",
    ] {
        assert!(
            parse_command(&[removed]).is_err(),
            "旧入口 {removed} 应被拒绝"
        );
        assert!(parse_command(&[removed, "--help"]).is_err());
        assert!(parse_command(&["help", removed]).is_err());
    }
}

#[test]
fn help_accepts_only_current_command_families() {
    assert_eq!(parse_command(&[]).unwrap(), Command::Help(None));
    assert_eq!(parse_command(&["--help"]).unwrap(), Command::Help(None));
    for command in ["dev", "check", "build", "generate", "data"] {
        let expected = Command::Help(Some(command.into()));
        assert_eq!(parse_command(&["help", command]).unwrap(), expected);
        assert_eq!(parse_command(&[command, "--help"]).unwrap(), expected);
    }
    assert!(parse_command(&["missing", "--help"]).is_err());
}

#[test]
fn parses_check_task_graph_and_internal_groups() {
    assert_eq!(
        parse_command(&["check", "--scope", "frontend", "--full", "--plan"]).unwrap(),
        Command::Check(CheckCommand::Run(CheckOptions {
            scope: CheckScope::Frontend,
            full: true,
            plan: true,
        }))
    );
    assert_eq!(
        parse_command(&["check", "doctor"]).unwrap(),
        Command::Check(CheckCommand::Doctor)
    );
    for (name, operation) in [
        ("plan", CiCommand::Plan),
        ("preflight", CiCommand::Preflight),
        ("rust-gate", CiCommand::RustGate),
        ("resource-gate", CiCommand::ResourceGate),
        ("integration", CiCommand::Integration),
        ("consumer-contract", CiCommand::ConsumerContract),
    ] {
        assert_eq!(
            parse_command(&["check", "ci", name]).unwrap(),
            Command::Check(CheckCommand::Ci(operation))
        );
    }
    assert!(parse_command(&["check", "ci"]).is_err());
    assert!(parse_command(&["check", "ci", "rust-gate", "extra"]).is_err());
    assert!(parse_command(&["check", "--full", "--full"]).is_err());
    assert!(parse_command(&["check", "--scope", "all", "--scope", "backend"]).is_err());
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
    assert!(matches!(
        parse_command(&[
            "check",
            "recovery",
            "fresh-target",
            "--workspace",
            "D:/隔离 target",
            "--operation",
            "status",
        ]),
        Ok(Command::Check(CheckCommand::Recovery(RecoveryCommand::FreshTarget(arguments))))
            if arguments == strings(&[
                "--workspace",
                "D:/隔离 target",
                "--operation",
                "status",
            ])
    ));
    for values in [
        ["check", "recovery", "source"].as_slice(),
        ["check", "recovery", "source", "unknown"].as_slice(),
        ["check", "recovery", "runtime", "unknown"].as_slice(),
        ["check", "recovery", "missing"].as_slice(),
    ] {
        assert!(parse_command(values).is_err());
    }
    assert_eq!(
        parse_command(&["check", "recovery", "fixture", "--output-dir", "fixture"]).unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::Fixture(strings(
            &["--output-dir", "fixture",]
        ))))
    );
    assert_eq!(
        parse_command(&[
            "check",
            "recovery",
            "dataset-prepare",
            "--plan",
            "reference.json",
        ])
        .unwrap(),
        Command::Check(CheckCommand::Recovery(RecoveryCommand::DatasetPrepare(
            strings(&["--plan", "reference.json"],)
        )))
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

#[test]
fn parses_build_and_development_options() {
    assert_eq!(
        parse_command(&["dev", "--measure-once"]).unwrap(),
        Command::Dev { measure_once: true }
    );
    assert_eq!(
        parse_command(&["build", "--real"]).unwrap(),
        Command::Build(BuildOptions {
            profile: BuildProfile::Release,
            real: true,
            plan: false,
        })
    );
    assert_eq!(
        parse_command(&["build", "--profile", "dev"]).unwrap(),
        Command::Build(BuildOptions {
            profile: BuildProfile::Dev,
            real: false,
            plan: false,
        })
    );
    assert_eq!(
        parse_command(&["build", "--real", "--profile", "release"]).unwrap(),
        Command::Build(BuildOptions {
            profile: BuildProfile::Release,
            real: true,
            plan: false,
        })
    );
    assert_eq!(
        parse_command(&["build", "--plan", "--profile", "dev", "--real"]).unwrap(),
        Command::Build(BuildOptions {
            profile: BuildProfile::Dev,
            real: true,
            plan: true,
        })
    );
    assert!(parse_command(&["build", "--profile", "test"]).is_err());
    assert!(parse_command(&["build", "--profile", "--real"]).is_err());
    assert!(parse_command(&["build", "--profile", "dev", "--profile", "release"]).is_err());
    assert!(parse_command(&["build", "--real", "--real"]).is_err());
    assert!(parse_command(&["build", "--plan", "--plan"]).is_err());
    assert!(parse_command(&["build", "--real", "extra"]).is_err());
    assert!(parse_command(&["dev", "--measure-once", "extra"]).is_err());
}

#[test]
fn generation_is_read_only_until_write_is_explicit() {
    assert_eq!(
        parse_command(&["generate", "api"]).unwrap(),
        Command::Generate(GenerateCommand::Api(ApiGenerateCommand {
            reference: None,
            write: false,
        }))
    );
    assert_eq!(
        parse_command(&["generate", "api", "--commit", "HEAD", "--write"]).unwrap(),
        Command::Generate(GenerateCommand::Api(ApiGenerateCommand {
            reference: Some("HEAD".into()),
            write: true,
        }))
    );
    assert!(parse_command(&["generate", "api", "--commit", "HEAD"]).is_err());
    assert!(parse_command(&["generate", "api", "--write", "--write"]).is_err());

    assert_eq!(
        parse_command(&["generate", "resource", "post"]).unwrap(),
        Command::Generate(GenerateCommand::Resource(ResourceCommand {
            target: ResourceTarget::Named("post".into()),
            action: ResourceAction::Preview,
        }))
    );
    assert_eq!(
        parse_command(&["generate", "resource", "--all", "--check"]).unwrap(),
        Command::Generate(GenerateCommand::Resource(ResourceCommand {
            target: ResourceTarget::All,
            action: ResourceAction::Check,
        }))
    );
    assert_eq!(
        parse_command(&["generate", "resource", "--all", "--write"]).unwrap(),
        Command::Generate(GenerateCommand::Resource(ResourceCommand {
            target: ResourceTarget::All,
            action: ResourceAction::Write,
        }))
    );
    assert!(parse_command(&["generate", "resource", "--all"]).is_err());
    assert!(parse_command(&["generate", "resource", "post", "--write", "--explain"]).is_err());
    for (flag, action) in [
        ("--write", ResourceAction::Write),
        ("--check", ResourceAction::Check),
        ("--explain", ResourceAction::Explain),
    ] {
        assert_eq!(
            parse_command(&["generate", "resource", "post", flag]).unwrap(),
            Command::Generate(GenerateCommand::Resource(ResourceCommand {
                target: ResourceTarget::Named("post".into()),
                action,
            }))
        );
    }
}

#[test]
fn data_groups_migrations_and_explicit_maintenance() {
    assert_eq!(
        parse_command(&["data", "migrate", "verify"]).unwrap(),
        Command::Data(DataCommand::Migrate(MigrationCommand::Run {
            operation: MigrationOperation::Verify,
            target: MigrationTarget::Control,
        }))
    );
    assert_eq!(
        parse_command(&["data", "migrate", "up", "tenant-data", "--all"]).unwrap(),
        Command::Data(DataCommand::Migrate(MigrationCommand::Run {
            operation: MigrationOperation::Up,
            target: MigrationTarget::TenantDataAll,
        }))
    );
    assert!(matches!(
        parse_command(&["data", "backup", "status"]),
        Ok(Command::Data(DataCommand::Backup(arguments))) if arguments == ["status"]
    ));
    assert!(matches!(
        parse_command(&["data", "restore", "verify", "--id", "r1"]),
        Ok(Command::Data(DataCommand::Restore(arguments))) if arguments[0] == "verify"
    ));
    assert!(matches!(
        parse_command(&["data", "target", "inventory", "--target", "tenant-a"]),
        Ok(Command::Data(DataCommand::TargetInventory(arguments)))
            if arguments == ["inventory", "--target", "tenant-a"]
    ));
    assert!(parse_command(&["data", "backup"]).is_err());
    assert!(parse_command(&["data", "file", "unknown"]).is_err());
    assert_eq!(
        parse_command(&["data", "migrate", "freeze"]).unwrap(),
        Command::Data(DataCommand::Migrate(MigrationCommand::Freeze))
    );
    assert_eq!(
        parse_command(&[
            "data",
            "migrate",
            "status",
            "tenant-data",
            "--target",
            "tenant-a"
        ])
        .unwrap(),
        Command::Data(DataCommand::Migrate(MigrationCommand::Run {
            operation: MigrationOperation::Status,
            target: MigrationTarget::TenantDataOne("tenant-a".into()),
        }))
    );
    assert!(parse_command(&["data", "migrate", "new", "unknown", "add_device"]).is_err());
}

#[test]
fn release_values_must_be_nonempty_and_cannot_consume_options() {
    let arguments = [
        "check",
        "release",
        "--tag",
        "v0.12.1",
        "--backend-repository",
        "owner/backend",
        "--backend-commit",
        "HEAD",
        "--frontend-repository",
        "owner/frontend",
        "--frontend-commit",
        "HEAD",
        "--manifest-path",
        "target/evidence.json",
    ];
    assert!(matches!(
        parse_command(&arguments),
        Ok(Command::Check(CheckCommand::Release(_)))
    ));
    for value in ["", " ", "-q", "--unknown"] {
        let mut invalid = arguments;
        invalid[3] = value;
        assert!(parse_command(&invalid).is_err(), "无效值：{value:?}");
    }
}

#[test]
fn global_frontend_dir_can_follow_command_arguments() {
    let cli = parse(strings(&[
        "check",
        "--plan",
        "--frontend-dir",
        "D:/工作区/frontend with spaces",
    ]))
    .unwrap();
    assert_eq!(
        cli.frontend_dir,
        PathBuf::from("D:/工作区/frontend with spaces")
    );
}

#[test]
fn rejects_ambiguous_global_or_value_arguments() {
    assert!(parse(strings(&["check", "--frontend-dir", "--full"])).is_err());
    assert!(parse(strings(&["check", "--frontend-dir", "-h"])).is_err());
    assert!(
        parse(strings(&[
            "check",
            "--frontend-dir",
            "a",
            "--frontend-dir",
            "b",
        ]))
        .is_err()
    );
    assert!(parse_command(&["generate", "api", "--commit", "-q", "--write"]).is_err());
    assert!(
        parse_command(&[
            "data",
            "migrate",
            "verify",
            "tenant-data",
            "--target",
            "--all",
        ])
        .is_err()
    );
}
