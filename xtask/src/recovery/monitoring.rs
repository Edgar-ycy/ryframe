use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{MonitoringBindOptions, MonitoringCommand},
    local_test_path::{
        LocalTestPathKind, validate_external_existing_file, validate_local_test_path,
    },
    process::run_with_env_removed,
};

const SCRIPT: &str = "scripts/restore_monitoring_delivery.py";
const PROTOCOL_KEY: &str = "RYFRAME_XTASK_RECOVERY_MONITORING";
const KIND: &str = "ryframe-xtask-recovery-monitoring";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateMonitoringInvocation {
    pub(crate) script: &'static str,
    pub(crate) protocol: String,
}

pub(super) fn run(command: &MonitoringCommand, root: &Path) -> Result<()> {
    if print_help(command) {
        return Ok(());
    }
    let invocation = private_invocation_at(command, root)?;
    run_with_env_removed(
        root,
        "python",
        &["-B", invocation.script],
        &[(PROTOCOL_KEY, invocation.protocol.as_str())],
        &[PROTOCOL_KEY],
    )
}

fn print_help(command: &MonitoringCommand) -> bool {
    let text = match command {
        MonitoringCommand::Help => {
            "cargo xtask check recovery monitoring <bind|start|observe|close|result|status> ..."
        }
        MonitoringCommand::BindHelp => {
            "cargo xtask check recovery monitoring bind --runtime-receipt <绝对文件> --target-plan <绝对文件> --output <绝对 binding.json> --run-id <标识> --metrics-token-file <绝对文件> --prometheus <绝对文件> --promtool <绝对文件> --alertmanager <绝对文件> --amtool <绝对文件> --prometheus-port <端口> --alertmanager-port <端口> --webhook-port <端口> --write"
        }
        MonitoringCommand::LifecycleHelp(operation) => {
            println!(
                "cargo xtask check recovery monitoring {} --binding <绝对文件>{}",
                operation.as_str(),
                if operation.writes() { " --write" } else { "" }
            );
            return true;
        }
        MonitoringCommand::Bind(_) | MonitoringCommand::Lifecycle { .. } => return false,
    };
    println!("{text}");
    true
}

pub(crate) fn private_invocation_at(
    command: &MonitoringCommand,
    root: &Path,
) -> Result<PrivateMonitoringInvocation> {
    let protocol = match command {
        MonitoringCommand::Bind(options) => bind_protocol(options, root)?,
        MonitoringCommand::Lifecycle { operation, binding } => {
            local(binding, root, LocalTestPathKind::ExistingFile)?;
            protocol(root, operation.as_str(), Some(binding), operation.writes())?
        }
        MonitoringCommand::Help
        | MonitoringCommand::BindHelp
        | MonitoringCommand::LifecycleHelp(_) => {
            return Err("monitoring 帮助不启动私有阶段程序".into());
        }
    };
    let serialized = serde_json::to_string(&protocol)?;
    if serialized.len() > 32 * 1024 || serialized.contains(['\n', '\r', '\0']) {
        return Err("monitoring 私有协议过长或包含换行符/NUL".into());
    }
    Ok(PrivateMonitoringInvocation {
        script: SCRIPT,
        protocol: serialized,
    })
}

fn bind_protocol(options: &MonitoringBindOptions, root: &Path) -> Result<Value> {
    for value in [
        &options.runtime_receipt,
        &options.target_plan,
        &options.metrics_token_file,
    ] {
        local(value, root, LocalTestPathKind::ExistingFile)?;
    }
    local(&options.output, root, LocalTestPathKind::OutputFile)?;
    for value in [
        &options.tools.prometheus,
        &options.tools.promtool,
        &options.tools.alertmanager,
        &options.tools.amtool,
    ] {
        validate_external_existing_file(value)?;
    }
    let mut value = protocol(root, "bind", None, true)?;
    value["runtime_receipt"] = json!(path_text(&options.runtime_receipt)?);
    value["target_plan"] = json!(path_text(&options.target_plan)?);
    value["output"] = json!(path_text(&options.output)?);
    value["run_id"] = json!(options.run_id);
    value["metrics_token_file"] = json!(path_text(&options.metrics_token_file)?);
    value["prometheus"] = json!(path_text(&options.tools.prometheus)?);
    value["promtool"] = json!(path_text(&options.tools.promtool)?);
    value["alertmanager"] = json!(path_text(&options.tools.alertmanager)?);
    value["amtool"] = json!(path_text(&options.tools.amtool)?);
    value["prometheus_port"] = json!(options.ports.prometheus);
    value["alertmanager_port"] = json!(options.ports.alertmanager);
    value["webhook_port"] = json!(options.ports.webhook);
    Ok(value)
}

fn protocol(root: &Path, operation: &str, binding: Option<&PathBuf>, write: bool) -> Result<Value> {
    Ok(json!({
        "alertmanager": null,
        "alertmanager_port": null,
        "amtool": null,
        "backend_dir": path_text(root)?,
        "binding": binding.map(|value| path_text(value)).transpose()?,
        "format_version": 1,
        "kind": KIND,
        "metrics_token_file": null,
        "operation": operation,
        "output": null,
        "prometheus": null,
        "prometheus_port": null,
        "promtool": null,
        "run_id": null,
        "runtime_receipt": null,
        "target_plan": null,
        "webhook_port": null,
        "write": write,
    }))
}

fn local(value: &Path, root: &Path, kind: LocalTestPathKind) -> Result<()> {
    validate_local_test_path(value, root, kind)
        .map(|_| ())
        .map_err(Into::into)
}

fn path_text(value: &Path) -> Result<&str> {
    value
        .to_str()
        .ok_or_else(|| "monitoring 路径必须能表示为 UTF-8".into())
}
