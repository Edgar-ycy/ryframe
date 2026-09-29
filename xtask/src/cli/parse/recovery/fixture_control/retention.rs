use crate::{
    cli::{CliError, FixtureRetentionCommand, FixtureRetentionOptions},
    local_test_path::LocalTestPathKind,
};

use super::{ParsedOptions, positive_i64, reject_write, require_write};

const COMMON_OPTIONS: &[&str] = &["--runtime-dir", "--tenant", "--migration"];
const HISTORY_OPTIONS: &[&str] = &["--runtime-dir", "--tenant", "--migration", "--plan-sha256"];

pub(super) fn parse(args: &[String]) -> Result<FixtureRetentionCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new("fixture retention 缺少明确操作"));
    };
    if matches!(operation.as_str(), "--help" | "-h") && values.is_empty() {
        return Ok(FixtureRetentionCommand::Help);
    }
    if is_help(values)
        && [
            "inspect",
            "plan-history",
            "historical-expired",
            "export-backup",
            "verify-cleaned",
        ]
        .contains(&operation.as_str())
    {
        return Ok(FixtureRetentionCommand::Help);
    }
    match operation.as_str() {
        "inspect" => readonly(values, "retention inspect").map(FixtureRetentionCommand::Inspect),
        "plan-history" => {
            readonly(values, "retention plan-history").map(FixtureRetentionCommand::PlanHistory)
        }
        "historical-expired" => historical_expired(values),
        "export-backup" => {
            writing(values, "retention export-backup").map(FixtureRetentionCommand::ExportBackup)
        }
        "verify-cleaned" => {
            readonly(values, "retention verify-cleaned").map(FixtureRetentionCommand::VerifyCleaned)
        }
        _ => Err(CliError::new(format!(
            "未知 fixture retention 操作：{operation}"
        ))),
    }
}

fn is_help(values: &[String]) -> bool {
    matches!(values, [help] if matches!(help.as_str(), "--help" | "-h"))
}

fn readonly(args: &[String], operation: &str) -> Result<FixtureRetentionOptions, CliError> {
    let options = ParsedOptions::parse(args, COMMON_OPTIONS)?;
    reject_write(&options, operation)?;
    common(&options)
}

fn writing(args: &[String], operation: &str) -> Result<FixtureRetentionOptions, CliError> {
    let options = ParsedOptions::parse(args, COMMON_OPTIONS)?;
    require_write(&options, operation)?;
    common(&options)
}

fn historical_expired(args: &[String]) -> Result<FixtureRetentionCommand, CliError> {
    let options = ParsedOptions::parse(args, HISTORY_OPTIONS)?;
    require_write(&options, "retention historical-expired")?;
    let plan_sha256 = valid_sha256(options.require("--plan-sha256")?)?;
    Ok(FixtureRetentionCommand::HistoricalExpired {
        options: common(&options)?,
        plan_sha256,
    })
}

fn common(options: &ParsedOptions) -> Result<FixtureRetentionOptions, CliError> {
    Ok(FixtureRetentionOptions {
        runtime_dir: options.path("--runtime-dir", LocalTestPathKind::ExistingDirectory)?,
        tenant: valid_tenant(options.require("--tenant")?)?,
        migration: positive_i64(options.require("--migration")?, "--migration")?,
    })
}

fn valid_tenant(value: &str) -> Result<String, CliError> {
    if value.len() == 15
        && value.starts_with("tenant-")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(value.to_owned())
    } else {
        Err(CliError::new("--tenant 必须符合 tenant-[a-f0-9]{8}"))
    }
}

fn valid_sha256(value: &str) -> Result<String, CliError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(value.to_owned())
    } else {
        Err(CliError::new("--plan-sha256 必须是 64 位小写十六进制摘要"))
    }
}
