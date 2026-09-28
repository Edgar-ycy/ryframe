use crate::{
    cli::{CliError, FixtureArtifactCommand, FixtureArtifactOptions},
    local_test_path::LocalTestPathKind,
};

use super::{ParsedOptions, output_file, positive_i64, reject_write};

const OPTIONS: &[&str] = &["--runtime-dir", "--job-id", "--receipt"];

pub(super) fn parse(args: &[String]) -> Result<FixtureArtifactCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new("fixture artifact 缺少明确操作"));
    };
    if matches!(operation.as_str(), "--help" | "-h") && values.is_empty() {
        return Ok(FixtureArtifactCommand::Help);
    }
    match operation.as_str() {
        "snapshot" if is_help(values) => Ok(FixtureArtifactCommand::Help),
        "snapshot" => parse_options(values, true).map(FixtureArtifactCommand::Snapshot),
        "verify-deleted" if is_help(values) => Ok(FixtureArtifactCommand::Help),
        "verify-deleted" => parse_options(values, false).map(FixtureArtifactCommand::VerifyDeleted),
        _ => Err(CliError::new(format!(
            "未知 fixture artifact 操作：{operation}"
        ))),
    }
}

fn is_help(values: &[String]) -> bool {
    matches!(values, [help] if matches!(help.as_str(), "--help" | "-h"))
}

fn parse_options(args: &[String], snapshot: bool) -> Result<FixtureArtifactOptions, CliError> {
    let options = ParsedOptions::parse(args, OPTIONS)?;
    reject_write(&options, "artifact")?;
    let receipt = if snapshot {
        output_file(options.require("--receipt")?, "--receipt")?
    } else {
        options.path("--receipt", LocalTestPathKind::ExistingFile)?
    };
    Ok(FixtureArtifactOptions {
        runtime_dir: options.path("--runtime-dir", LocalTestPathKind::ExistingDirectory)?,
        job_id: positive_i64(options.require("--job-id")?, "--job-id")?,
        receipt,
    })
}
