use std::path::{Path, PathBuf};

use super::super::model::{CliError, FullStackCommand, RecoveryCommand};

#[path = "recovery/clone.rs"]
mod clone;
#[path = "recovery/dataset_prepare.rs"]
mod dataset_prepare;
#[path = "recovery/fixture_control.rs"]
mod fixture_control;
#[path = "recovery/fixture_prepare.rs"]
mod fixture_prepare;
#[path = "recovery/fixture_runtime.rs"]
mod fixture_runtime;
#[path = "recovery/fresh_target.rs"]
mod fresh_target;
#[path = "recovery/inputs.rs"]
mod inputs;
#[path = "recovery/monitoring.rs"]
mod monitoring;
#[path = "recovery/reference.rs"]
mod reference;
#[path = "recovery/runtime.rs"]
mod runtime;
#[path = "recovery/runtime_values.rs"]
mod runtime_values;
#[path = "recovery/seed_source.rs"]
mod seed_source;
#[path = "recovery/source.rs"]
mod source;

use fixture_control::parse_fixture_control;
use fixture_runtime::parse_fixture_runtime;

pub(super) fn parse_recovery(args: &[String]) -> Result<RecoveryCommand, CliError> {
    let Some((stage, rest)) = args.split_first() else {
        return Err(CliError::new("check recovery 缺少明确阶段"));
    };
    match stage.as_str() {
        "plan" | "check-dataset" | "check-existing" | "dataset" | "backup" | "restore" | "copy"
        | "damage" => reference::parse(args).map(RecoveryCommand::Reference),
        "inputs" => inputs::parse(rest).map(RecoveryCommand::Inputs),
        "runtime" => runtime::parse(rest).map(RecoveryCommand::Runtime),
        "source" => source::parse(rest).map(RecoveryCommand::Source),
        "clone" => parse_clone(rest),
        "fresh-target" => fresh_target::parse(rest).map(RecoveryCommand::FreshTarget),
        "fixture" => parse_fixture(rest),
        "full-stack" => parse_full_stack(rest).map(RecoveryCommand::FullStack),
        "monitoring" => monitoring::parse(rest).map(RecoveryCommand::Monitoring),
        "dataset-prepare" => dataset_prepare::parse(rest).map(RecoveryCommand::DatasetPrepare),
        _ => Err(CliError::new(format!("未知 recovery 阶段：{stage}"))),
    }
}

fn parse_clone(args: &[String]) -> Result<RecoveryCommand, CliError> {
    let Some(operation) = args.first() else {
        return Err(CliError::new("check recovery clone 缺少明确子操作"));
    };
    let seed_help = operation == "seed-runtime"
        && args[1..]
            .iter()
            .any(|value| matches!(value.as_str(), "--help" | "-h"));
    if operation == "seed-runtime"
        && !seed_help
        && seed_source::selects_source_operation(&args[1..])?
    {
        return seed_source::parse(&args[1..]).map(RecoveryCommand::SeedSource);
    }
    clone::parse(args).map(RecoveryCommand::Clone)
}

fn parse_full_stack(args: &[String]) -> Result<FullStackCommand, CliError> {
    match args {
        [help] if matches!(help.as_str(), "--help" | "-h") => Ok(FullStackCommand::Help),
        [operation] if operation == "prepare" => Ok(FullStackCommand::Prepare),
        [operation] if operation == "start" => Ok(FullStackCommand::Start),
        [operation] if operation == "collect" => Ok(FullStackCommand::Collect),
        [operation, help]
            if matches!(operation.as_str(), "prepare" | "start" | "collect")
                && matches!(help.as_str(), "--help" | "-h") =>
        {
            Ok(FullStackCommand::Help)
        }
        [operation, help]
            if operation == "rate-limit" && matches!(help.as_str(), "--help" | "-h") =>
        {
            Ok(FullStackCommand::RateLimitHelp)
        }
        [operation, option, value]
            if operation == "rate-limit" && option == "--environment-file" =>
        {
            validate_environment_file(value)
                .map(|environment_file| FullStackCommand::RateLimit { environment_file })
        }
        [] => Err(CliError::new("check recovery full-stack 缺少明确子操作")),
        [operation, ..]
            if ["prepare", "start", "collect", "rate-limit"].contains(&operation.as_str()) =>
        {
            Err(CliError::new(format!("full-stack {operation} 参数无效")))
        }
        [operation, ..] => Err(CliError::new(format!(
            "未知 recovery full-stack 子操作：{operation}"
        ))),
    }
}

fn validate_environment_file(value: &str) -> Result<PathBuf, CliError> {
    if value.trim().is_empty() || value.contains(['\r', '\n']) {
        return Err(CliError::new("--environment-file 必须是无换行的绝对路径"));
    }
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(CliError::new("--environment-file 必须是无换行的绝对路径"));
    }
    Ok(path.to_path_buf())
}

fn parse_fixture(args: &[String]) -> Result<RecoveryCommand, CliError> {
    match args {
        [option, ..] if option.starts_with('-') => {
            fixture_prepare::parse(args).map(RecoveryCommand::FixturePrepare)
        }
        [operation, rest @ ..] if operation == "runtime" => {
            parse_fixture_runtime(rest).map(RecoveryCommand::FixtureRuntime)
        }
        [operation, rest @ ..]
            if [
                "environment",
                "review",
                "request",
                "source-pair",
                "successor",
                "services",
                "artifact",
            ]
            .contains(&operation.as_str()) =>
        {
            parse_fixture_control(operation, rest)
                .map(Box::new)
                .map(RecoveryCommand::FixtureControl)
        }
        [operation, ..] if ["retention", "dataset"].contains(&operation.as_str()) => {
            Ok(RecoveryCommand::Fixture(args.to_vec()))
        }
        _ => Err(CliError::new(
            "用法：cargo xtask check recovery fixture --output-dir <目录> --write，或 fixture <environment|review|request|successor|artifact|retention|services|source-pair|runtime|dataset> ...",
        )),
    }
}

fn valid_commit_sha(value: &str) -> bool {
    value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
