use std::path::{Path, PathBuf};

use super::super::model::{CliError, FullStackCommand, RecoveryCommand};

#[path = "recovery/fresh_target.rs"]
mod fresh_target;
#[path = "recovery/fixture_runtime.rs"]
mod fixture_runtime;
use fixture_runtime::parse_fixture_runtime;

pub(super) fn parse_recovery(args: &[String]) -> Result<RecoveryCommand, CliError> {
    let Some((stage, rest)) = args.split_first() else {
        return Err(CliError::new("check recovery 缺少明确阶段"));
    };
    match stage.as_str() {
        "plan" | "check-dataset" | "check-existing" | "dataset" | "backup" | "restore" | "copy"
        | "damage" => Ok(RecoveryCommand::Reference(args.to_vec())),
        "inputs" => parse_recovery_operation("inputs", rest, &["reference", "product", "bindings"])
            .map(RecoveryCommand::Inputs),
        "runtime" => parse_recovery_operation(
            "runtime",
            rest,
            &[
                "build", "register", "start", "status", "stop", "recover", "bind", "verify",
            ],
        )
        .map(RecoveryCommand::Runtime),
        "source" => parse_recovery_operation(
            "source",
            rest,
            &["verify", "comparison-capture", "comparison-verify"],
        )
        .map(RecoveryCommand::Source),
        "clone" => parse_recovery_operation(
            "clone",
            rest,
            &[
                "plan",
                "verify",
                "init",
                "status",
                "stage",
                "runtime",
                "recover",
                "recover-copy",
                "bridge",
                "post-copy",
                "seed-runtime",
                "storage",
                "cache",
                "maintenance",
            ],
        )
        .map(RecoveryCommand::Clone),
        "fresh-target" => fresh_target::parse(rest).map(RecoveryCommand::FreshTarget),
        "fixture" => parse_fixture(rest),
        "full-stack" => parse_full_stack(rest).map(RecoveryCommand::FullStack),
        "monitoring" => parse_monitoring(rest).map(RecoveryCommand::Monitoring),
        "dataset-prepare" => Ok(RecoveryCommand::DatasetPrepare(rest.to_vec())),
        _ => Err(CliError::new(format!("未知 recovery 阶段：{stage}"))),
    }
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

fn parse_monitoring(args: &[String]) -> Result<Vec<String>, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new("check recovery monitoring 缺少明确子操作"));
    };
    if args.len() == 1 && matches!(operation.as_str(), "--help" | "-h") {
        return Ok(args.to_vec());
    }
    if !["bind", "start", "observe", "close", "result", "status"].contains(&operation.as_str()) {
        return Err(CliError::new(format!(
            "未知 recovery monitoring 子操作：{operation}"
        )));
    }
    if values
        .iter()
        .any(|value| matches!(value.as_str(), "--help" | "-h"))
    {
        return Ok(args.to_vec());
    }
    validate_monitoring_options(operation, values)?;
    Ok(args.to_vec())
}

fn validate_monitoring_options(operation: &str, args: &[String]) -> Result<(), CliError> {
    let named: &[&str] = if operation == "bind" {
        &[
            "--runtime-receipt",
            "--target-plan",
            "--output",
            "--run-id",
            "--metrics-token-file",
            "--prometheus",
            "--promtool",
            "--alertmanager",
            "--amtool",
            "--prometheus-port",
            "--alertmanager-port",
            "--webhook-port",
        ]
    } else {
        &["--binding"]
    };
    let mut observed = std::collections::BTreeMap::new();
    let mut write = false;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if option == "--write" {
            if write {
                return Err(CliError::new("--write 不能重复"));
            }
            write = true;
            index += 1;
            continue;
        }
        if !named.contains(&option) {
            return Err(CliError::new(format!("未知 monitoring 参数：{option}")));
        }
        let value = args
            .get(index + 1)
            .filter(|value| !value.trim().is_empty() && !value.starts_with('-'))
            .ok_or_else(|| CliError::new(format!("{option} 缺少取值")))?;
        if observed.insert(option, value).is_some() {
            return Err(CliError::new(format!("{option} 不能重复")));
        }
        index += 2;
    }
    if let Some(missing) = named.iter().find(|name| !observed.contains_key(**name)) {
        return Err(CliError::new(format!(
            "monitoring {operation} 缺少必需参数 {missing}"
        )));
    }
    if operation == "status" {
        if write {
            return Err(CliError::new(
                "monitoring status 是只读操作，不接受 --write",
            ));
        }
    } else if !write {
        return Err(CliError::new(format!(
            "monitoring {operation} 会发布阶段证据，必须显式传入 --write"
        )));
    }
    if operation == "bind" {
        validate_monitoring_ports(&observed)?;
    }
    Ok(())
}

fn validate_monitoring_ports(
    values: &std::collections::BTreeMap<&str, &String>,
) -> Result<(), CliError> {
    let mut ports = Vec::new();
    for name in ["--prometheus-port", "--alertmanager-port", "--webhook-port"] {
        let value = values[name]
            .parse::<u16>()
            .map_err(|_| CliError::new(format!("{name} 必须是 1024 到 65535 的端口")))?;
        if value < 1024 {
            return Err(CliError::new(format!("{name} 必须是 1024 到 65535 的端口")));
        }
        ports.push(value);
    }
    if ports
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != ports.len()
    {
        return Err(CliError::new("三个 monitoring 端口必须互异"));
    }
    Ok(())
}

fn parse_fixture(args: &[String]) -> Result<RecoveryCommand, CliError> {
    match args {
        [option] if matches!(option.as_str(), "--help" | "-h") => {
            Ok(RecoveryCommand::Fixture(args.to_vec()))
        }
        [option, ..] if option.starts_with("--") => {
            validate_fixture_prepare(args)?;
            Ok(RecoveryCommand::Fixture(args.to_vec()))
        }
        [operation, rest @ ..] if operation == "runtime" => {
            parse_fixture_runtime(rest).map(RecoveryCommand::FixtureRuntime)
        }
        [operation, ..]
            if [
                "environment",
                "review",
                "request",
                "successor",
                "artifact",
                "retention",
                "services",
                "source-pair",
                "dataset",
            ]
            .contains(&operation.as_str()) =>
        {
            Ok(RecoveryCommand::Fixture(args.to_vec()))
        }
        _ => Err(CliError::new(
            "用法：cargo xtask check recovery fixture --output-dir <目录> --write，或 fixture <environment|review|request|successor|artifact|retention|services|source-pair|runtime|dataset> ...",
        )),
    }
}

fn validate_fixture_prepare(args: &[String]) -> Result<(), CliError> {
    let mut values = std::collections::BTreeMap::new();
    let mut write = false;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if option == "--write" {
            if write {
                return Err(CliError::new("--write 不能重复"));
            }
            write = true;
            index += 1;
            continue;
        }
        if ![
            "--output-dir",
            "--expected-backend-sha",
            "--expected-frontend-sha",
        ]
        .contains(&option)
        {
            return Err(CliError::new(format!("未知 fixture 参数：{option}")));
        }
        let value = args
            .get(index + 1)
            .filter(|value| !value.trim().is_empty() && !value.starts_with('-'))
            .ok_or_else(|| CliError::new(format!("{option} 缺少取值")))?;
        if values.insert(option, value).is_some() {
            return Err(CliError::new(format!("{option} 不能重复")));
        }
        index += 2;
    }
    if !values.contains_key("--output-dir") {
        return Err(CliError::new("fixture 缺少必需参数 --output-dir"));
    }
    if !write {
        return Err(CliError::new("生成 Device 夹具必须显式传入 --write"));
    }
    let backend = values.get("--expected-backend-sha");
    let frontend = values.get("--expected-frontend-sha");
    if backend.is_some() != frontend.is_some() {
        return Err(CliError::new(
            "正式夹具必须同时指定 --expected-backend-sha 与 --expected-frontend-sha",
        ));
    }
    if [backend, frontend]
        .into_iter()
        .flatten()
        .any(|value| !valid_commit_sha(value))
    {
        return Err(CliError::new(
            "正式夹具提交必须是非零的 40 位小写十六进制 SHA",
        ));
    }
    Ok(())
}

fn valid_commit_sha(value: &str) -> bool {
    value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn parse_recovery_operation(
    stage: &str,
    args: &[String],
    operations: &[&str],
) -> Result<Vec<String>, CliError> {
    let Some(operation) = args.first() else {
        return Err(CliError::new(format!(
            "check recovery {stage} 缺少明确子操作"
        )));
    };
    if args.len() == 1 && matches!(operation.as_str(), "--help" | "-h") {
        return Ok(args.to_vec());
    }
    if !operations.contains(&operation.as_str()) {
        return Err(CliError::new(format!(
            "未知 recovery {stage} 子操作：{operation}"
        )));
    }
    Ok(args.to_vec())
}
