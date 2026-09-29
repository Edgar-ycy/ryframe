use crate::{
    cli::{CliError, FixtureEnvironmentCommand, FixtureEnvironmentInputs, FixtureFileOutput},
    local_test_path::LocalTestPathKind,
};

use super::{ParsedOptions, new_directory, parse_side, reject_write, require_write};

const INPUTS: &[&str] = &["--review", "--fixture", "--maintenance-build", "--side"];

pub(super) fn parse(args: &[String]) -> Result<FixtureEnvironmentCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new("fixture environment 缺少明确操作"));
    };
    if matches!(operation.as_str(), "--help" | "-h") && values.is_empty() {
        return Ok(FixtureEnvironmentCommand::Help);
    }
    if matches!(values, [help] if matches!(help.as_str(), "--help" | "-h")) {
        return Ok(FixtureEnvironmentCommand::Help);
    }
    match operation.as_str() {
        "plan" => parse_plan(values),
        "prepare" => parse_prepare(values),
        "review" => parse_review(values),
        "rotate-secrets" => parse_rotate(values),
        "bootstrap-secrets" => parse_bootstrap(values),
        _ => Err(CliError::new(format!(
            "未知 fixture environment 操作：{operation}"
        ))),
    }
}

fn parse_plan(args: &[String]) -> Result<FixtureEnvironmentCommand, CliError> {
    let options = ParsedOptions::parse(args, INPUTS)?;
    reject_write(&options, "environment plan")?;
    environment_inputs(&options).map(FixtureEnvironmentCommand::Plan)
}

fn parse_prepare(args: &[String]) -> Result<FixtureEnvironmentCommand, CliError> {
    let options = ParsedOptions::parse(
        args,
        &[
            "--review",
            "--fixture",
            "--maintenance-build",
            "--side",
            "--output",
            "--secrets-dir",
        ],
    )?;
    require_write(&options, "environment prepare")?;
    let inputs = environment_inputs(&options)?;
    let output = new_directory(options.require("--output")?, "--output")?;
    let secrets_dir =
        options.optional_path("--secrets-dir", LocalTestPathKind::ExistingDirectory)?;
    Ok(FixtureEnvironmentCommand::Prepare {
        inputs,
        output,
        secrets_dir,
    })
}

fn parse_review(args: &[String]) -> Result<FixtureEnvironmentCommand, CliError> {
    let options = ParsedOptions::parse(args, &["--review", "--output"])?;
    require_write(&options, "environment review")?;
    Ok(FixtureEnvironmentCommand::Review {
        review: options.path("--review", LocalTestPathKind::ExistingFile)?,
        output: FixtureFileOutput {
            output: super::output_file(options.require("--output")?, "--output")?,
        },
    })
}

fn parse_rotate(args: &[String]) -> Result<FixtureEnvironmentCommand, CliError> {
    let options = ParsedOptions::parse(args, &["--fixture", "--output"])?;
    require_write(&options, "environment rotate-secrets")?;
    Ok(FixtureEnvironmentCommand::RotateSecrets {
        fixture: options.path("--fixture", LocalTestPathKind::ExistingFile)?,
        output: new_directory(options.require("--output")?, "--output")?,
    })
}

fn parse_bootstrap(args: &[String]) -> Result<FixtureEnvironmentCommand, CliError> {
    let options =
        ParsedOptions::parse(args, &["--source-fixture", "--source-secrets", "--fixture"])?;
    require_write(&options, "environment bootstrap-secrets")?;
    Ok(FixtureEnvironmentCommand::BootstrapSecrets {
        source_fixture: options.path("--source-fixture", LocalTestPathKind::ExistingFile)?,
        source_secrets: options.path("--source-secrets", LocalTestPathKind::ExistingDirectory)?,
        fixture: options.path("--fixture", LocalTestPathKind::ExistingFile)?,
    })
}

fn environment_inputs(options: &ParsedOptions) -> Result<FixtureEnvironmentInputs, CliError> {
    Ok(FixtureEnvironmentInputs {
        review: options.path("--review", LocalTestPathKind::ExistingFile)?,
        fixture: options.path("--fixture", LocalTestPathKind::ExistingFile)?,
        maintenance_build: options.path("--maintenance-build", LocalTestPathKind::ExistingFile)?,
        side: parse_side(options.optional("--side").unwrap_or("seed"), true)?,
    })
}
