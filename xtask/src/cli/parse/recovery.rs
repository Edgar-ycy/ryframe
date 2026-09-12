use super::super::model::{CliError, RecoveryCommand};

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
        "fresh-target" => Ok(RecoveryCommand::FreshTarget(rest.to_vec())),
        "fixture" => parse_fixture(rest).map(RecoveryCommand::Fixture),
        "monitoring" => parse_monitoring(rest).map(RecoveryCommand::Monitoring),
        "dataset-prepare" => Ok(RecoveryCommand::DatasetPrepare(rest.to_vec())),
        _ => Err(CliError::new(format!("未知 recovery 阶段：{stage}"))),
    }
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

fn parse_fixture(args: &[String]) -> Result<Vec<String>, CliError> {
    match args {
        [option, ..] if option.starts_with("--") => Ok(args.to_vec()),
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
                "runtime",
                "dataset",
            ]
            .contains(&operation.as_str()) =>
        {
            Ok(args.to_vec())
        }
        _ => Err(CliError::new(
            "用法：cargo xtask check recovery fixture --output-dir <目录> --write，或 fixture <environment|review|request|successor|artifact|retention|services|source-pair|runtime|dataset> ...",
        )),
    }
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
