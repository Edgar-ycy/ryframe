use std::{collections::BTreeMap, path::PathBuf};

use crate::{
    local_test_path::{
        LocalTestPathKind, validate_external_existing_file, validate_local_test_path,
    },
    workspace::root_dir,
};

use super::super::super::model::{
    CliError, MonitoringBindOptions, MonitoringCommand, MonitoringOperation, MonitoringPorts,
    MonitoringTools,
};

const BIND_OPTIONS: [&str; 12] = [
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
];

pub(super) fn parse(args: &[String]) -> Result<MonitoringCommand, CliError> {
    match args {
        [help] if matches!(help.as_str(), "--help" | "-h") => return Ok(MonitoringCommand::Help),
        [operation, help] if matches!(help.as_str(), "--help" | "-h") => {
            return if operation == "bind" {
                Ok(MonitoringCommand::BindHelp)
            } else {
                parse_operation(operation).map(MonitoringCommand::LifecycleHelp)
            };
        }
        _ if args
            .iter()
            .any(|value| matches!(value.as_str(), "--help" | "-h")) =>
        {
            return Err(CliError::new("monitoring 帮助不能与执行参数同时使用"));
        }
        _ => {}
    }
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new("check recovery monitoring 缺少明确子操作"));
    };
    if operation == "bind" {
        parse_bind(values).map(|options| MonitoringCommand::Bind(Box::new(options)))
    } else {
        parse_lifecycle(parse_operation(operation)?, values)
    }
}

fn parse_operation(value: &str) -> Result<MonitoringOperation, CliError> {
    match value {
        "start" => Ok(MonitoringOperation::Start),
        "observe" => Ok(MonitoringOperation::Observe),
        "close" => Ok(MonitoringOperation::Close),
        "result" => Ok(MonitoringOperation::Result),
        "status" => Ok(MonitoringOperation::Status),
        _ => Err(CliError::new(format!(
            "未知 recovery monitoring 子操作：{value}"
        ))),
    }
}

fn parse_bind(args: &[String]) -> Result<MonitoringBindOptions, CliError> {
    let parsed = parse_named(args, &BIND_OPTIONS)?;
    if !parsed.write {
        return Err(CliError::new(
            "monitoring bind 会发布阶段证据，必须显式传入 --write",
        ));
    }
    let root = root_dir();
    let output = local(
        required(&parsed.values, "--output")?,
        "--output",
        LocalTestPathKind::OutputFile,
        &root,
    )?;
    if output.file_name().and_then(|value| value.to_str()) != Some("binding.json") {
        return Err(CliError::new("--output 必须命名为 binding.json"));
    }
    let output_parent = output
        .parent()
        .ok_or_else(|| CliError::new("--output 缺少运行目录"))?;
    validate_local_test_path(output_parent, &root, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| CliError::new(format!("--output 父目录无效：{error}")))?;
    let metrics_token_file = local(
        required(&parsed.values, "--metrics-token-file")?,
        "--metrics-token-file",
        LocalTestPathKind::ExistingFile,
        &root,
    )?;
    if metrics_token_file.parent() != Some(output_parent) {
        return Err(CliError::new(
            "--metrics-token-file 必须直接位于 binding.json 的运行目录",
        ));
    }
    if metrics_token_file
        .file_name()
        .and_then(|value| value.to_str())
        != Some("metrics-token.txt")
    {
        return Err(CliError::new(
            "--metrics-token-file 必须命名为 metrics-token.txt",
        ));
    }
    Ok(MonitoringBindOptions {
        runtime_receipt: local(
            required(&parsed.values, "--runtime-receipt")?,
            "--runtime-receipt",
            LocalTestPathKind::ExistingFile,
            &root,
        )?,
        target_plan: local(
            required(&parsed.values, "--target-plan")?,
            "--target-plan",
            LocalTestPathKind::ExistingFile,
            &root,
        )?,
        output,
        run_id: run_id(required(&parsed.values, "--run-id")?)?,
        metrics_token_file,
        tools: MonitoringTools {
            prometheus: external(&parsed.values, "--prometheus")?,
            promtool: external(&parsed.values, "--promtool")?,
            alertmanager: external(&parsed.values, "--alertmanager")?,
            amtool: external(&parsed.values, "--amtool")?,
        },
        ports: ports(&parsed.values)?,
    })
}

fn parse_lifecycle(
    operation: MonitoringOperation,
    args: &[String],
) -> Result<MonitoringCommand, CliError> {
    let parsed = parse_named(args, &["--binding"])?;
    if parsed.write != operation.writes() {
        return Err(CliError::new(if operation.writes() {
            format!(
                "monitoring {} 会发布阶段证据，必须显式传入 --write",
                operation.as_str()
            )
        } else {
            "monitoring status 是只读操作，不接受 --write".to_owned()
        }));
    }
    let binding = local(
        required(&parsed.values, "--binding")?,
        "--binding",
        LocalTestPathKind::ExistingFile,
        &root_dir(),
    )?;
    Ok(MonitoringCommand::Lifecycle { operation, binding })
}

struct ParsedOptions<'a> {
    values: BTreeMap<&'static str, &'a str>,
    write: bool,
}

fn parse_named<'a>(
    args: &'a [String],
    allowed: &[&'static str],
) -> Result<ParsedOptions<'a>, CliError> {
    let mut values = BTreeMap::new();
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
        let Some(canonical) = allowed.iter().find(|known| **known == option) else {
            return Err(CliError::new(format!("未知 monitoring 参数：{option}")));
        };
        let value = args
            .get(index + 1)
            .filter(|value| !value.trim().is_empty() && !value.starts_with('-'))
            .ok_or_else(|| CliError::new(format!("{option} 缺少取值")))?;
        if values.insert(*canonical, value.as_str()).is_some() {
            return Err(CliError::new(format!("{option} 不能重复")));
        }
        index += 2;
    }
    Ok(ParsedOptions { values, write })
}

fn required<'a>(values: &BTreeMap<&str, &'a str>, name: &str) -> Result<&'a str, CliError> {
    values
        .get(name)
        .copied()
        .ok_or_else(|| CliError::new(format!("monitoring 缺少必需参数 {name}")))
}

fn local(
    value: &str,
    name: &str,
    kind: LocalTestPathKind,
    root: &std::path::Path,
) -> Result<PathBuf, CliError> {
    validate_local_test_path(&PathBuf::from(value), root, kind)
        .map_err(|error| CliError::new(format!("{name} 无效：{error}")))
}

fn external(values: &BTreeMap<&str, &str>, name: &str) -> Result<PathBuf, CliError> {
    validate_external_existing_file(&PathBuf::from(required(values, name)?))
        .map_err(|error| CliError::new(format!("{name} 无效：{error}")))
}

fn run_id(value: &str) -> Result<String, CliError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || (index > 0 && byte.is_ascii_digit())
                || (index > 0 && byte == b'-')
        })
        && !value.ends_with('-')
        && !value.contains("--");
    if valid {
        Ok(value.to_owned())
    } else {
        Err(CliError::new(
            "--run-id 必须是最长 64 位的小写字母、数字及单连字符标识",
        ))
    }
}

fn ports(values: &BTreeMap<&str, &str>) -> Result<MonitoringPorts, CliError> {
    let parse = |name| -> Result<u16, CliError> {
        let value = required(values, name)?
            .parse::<u16>()
            .map_err(|_| CliError::new(format!("{name} 必须是 1024 到 65535 的端口")))?;
        if value < 1024 {
            return Err(CliError::new(format!("{name} 必须是 1024 到 65535 的端口")));
        }
        Ok(value)
    };
    let ports = MonitoringPorts {
        prometheus: parse("--prometheus-port")?,
        alertmanager: parse("--alertmanager-port")?,
        webhook: parse("--webhook-port")?,
    };
    if ports.prometheus == ports.alertmanager
        || ports.prometheus == ports.webhook
        || ports.alertmanager == ports.webhook
    {
        return Err(CliError::new("三个 monitoring 端口必须互异"));
    }
    Ok(ports)
}
