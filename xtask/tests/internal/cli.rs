use std::path::PathBuf;

use super::cli::{
    ApiSyncCommand, CheckScope, CiCommand, CliError, Command, ContractOperation, MigrationCommand,
    MigrationOperation, MigrationTarget, ResourceAction, ResourceTarget, parse,
};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn parses_ci_internal_commands_exactly() {
    assert_eq!(
        parse_command(&["ci", "plan"]).unwrap(),
        Command::Ci(CiCommand::Plan)
    );
    assert_eq!(
        parse_command(&["ci", "preflight"]).unwrap(),
        Command::Ci(CiCommand::Preflight)
    );
    assert_eq!(
        parse_command(&["ci", "rust-gate"]).unwrap(),
        Command::Ci(CiCommand::RustGate)
    );
    assert_eq!(
        parse_command(&["ci", "integration"]).unwrap(),
        Command::Ci(CiCommand::Integration)
    );
    assert_eq!(
        parse_command(&["ci", "consumer-contract"]).unwrap(),
        Command::Ci(CiCommand::ConsumerContract)
    );
    assert!(parse_command(&["ci"]).is_err());
    assert!(parse_command(&["ci", "rust-gate", "extra"]).is_err());
}

fn parse_command(values: &[&str]) -> std::result::Result<Command, CliError> {
    parse(strings(values)).map(|cli| cli.command)
}

#[test]
fn preserves_read_only_legacy_check_commands() {
    assert_eq!(
        parse_command(&["check", "--scope", "backend"]).unwrap(),
        Command::Check {
            scope: CheckScope::Backend
        }
    );
    assert_eq!(
        parse_command(&["contract", "check"]).unwrap(),
        Command::Contract {
            operation: ContractOperation::Check
        }
    );
    assert!(parse_command(&["contract", "sync"]).is_err());
}

#[test]
fn parses_daily_short_commands() {
    assert_eq!(
        parse_command(&["verify", "--full", "--scope", "frontend"]).unwrap(),
        Command::Verify {
            scope: CheckScope::Frontend,
            full: true
        }
    );
    let Command::Resource(resource) = parse_command(&["resource", "post", "--explain"]).unwrap()
    else {
        panic!("应解析为资源命令");
    };
    assert_eq!(resource.target, ResourceTarget::Named("post".into()));
    assert_eq!(resource.action, ResourceAction::Explain);
    assert_eq!(
        parse_command(&["api-sync", "--commit", "HEAD"]).unwrap(),
        Command::ApiSync(ApiSyncCommand::Commit("HEAD".into()))
    );
    assert_eq!(
        parse_command(&["migrate", "freeze"]).unwrap(),
        Command::Migrate(MigrationCommand::Freeze)
    );
}

#[test]
fn migration_defaults_to_control_scope() {
    assert_eq!(
        parse_command(&["migrate", "verify"]).unwrap(),
        Command::Migrate(MigrationCommand::Run {
            operation: MigrationOperation::Verify,
            target: MigrationTarget::Control,
        })
    );
    assert_eq!(
        parse_command(&["migrate", "up", "tenant-data", "--all"]).unwrap(),
        Command::Migrate(MigrationCommand::Run {
            operation: MigrationOperation::Up,
            target: MigrationTarget::TenantDataAll,
        })
    );
}

#[test]
fn global_frontend_dir_can_follow_command_arguments() {
    let cli = parse(strings(&[
        "contract",
        "check",
        "--frontend-dir",
        "D:/workspace/frontend",
    ]))
    .unwrap();
    assert_eq!(cli.frontend_dir, PathBuf::from("D:/workspace/frontend"));
}

#[test]
fn rejects_ambiguous_or_duplicate_arguments() {
    assert!(parse_command(&["resource", "post", "--write", "--explain"]).is_err());
    assert!(parse_command(&["resource", "--all"]).is_err());
    assert!(parse_command(&["resource", "--all", "--write"]).is_err());
    assert!(parse_command(&["verify", "--scope", "all", "--scope", "backend"]).is_err());
    assert!(parse_command(&["migrate", "new", "unknown", "add_device"]).is_err());
    assert!(parse(strings(&["verify", "--frontend-dir", "--full"])).is_err());
    assert!(parse_command(&["api-sync", "--commit", "--full"]).is_err());
    assert!(parse_command(&["api-sync", "--commit", "-q"]).is_err());
    assert!(parse_command(&["migrate", "verify", "tenant-data", "--target", "--all"]).is_err());
}

#[test]
fn daily_commands_forward_every_supported_argument() {
    assert_eq!(
        parse_command(&["verify", "--scope", "backend", "--full"]).unwrap(),
        Command::Verify {
            scope: CheckScope::Backend,
            full: true,
        }
    );
    assert!(matches!(
        parse_command(&["resource", "post"]).unwrap(),
        Command::Resource(command)
            if command.target == ResourceTarget::Named("post".into())
                && command.action == ResourceAction::Preview
    ));
    assert!(matches!(
        parse_command(&["resource", "post", "--write"]).unwrap(),
        Command::Resource(command)
            if command.target == ResourceTarget::Named("post".into())
                && command.action == ResourceAction::Write
    ));
    assert!(matches!(
        parse_command(&["resource", "post", "--check"]).unwrap(),
        Command::Resource(command)
            if command.target == ResourceTarget::Named("post".into())
                && command.action == ResourceAction::Check
    ));
    assert_eq!(
        parse_command(&["resource", "--all", "--check"]).unwrap(),
        Command::Resource(super::cli::ResourceCommand {
            target: ResourceTarget::All,
            action: ResourceAction::Check,
        })
    );
    assert_eq!(
        parse_command(&["migrate", "status", "tenant-data", "--target", "tenant-a"]).unwrap(),
        Command::Migrate(MigrationCommand::Run {
            operation: MigrationOperation::Status,
            target: MigrationTarget::TenantDataOne("tenant-a".into()),
        })
    );
}
