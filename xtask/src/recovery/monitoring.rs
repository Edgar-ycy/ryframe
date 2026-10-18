use std::path::Path;

use serde_json::json;

use crate::{
    Result,
    cli::{MonitoringCommand, MonitoringOperation},
};

#[path = "monitoring/bind.rs"]
pub(crate) mod bind;
#[path = "monitoring/binding.rs"]
pub(crate) mod binding;
#[path = "monitoring/config.rs"]
pub(crate) mod config;
#[path = "monitoring/contract.rs"]
pub(crate) mod contract;
#[path = "monitoring/execution.rs"]
pub(crate) mod execution;
#[path = "monitoring/failure.rs"]
pub(crate) mod failure;
#[path = "monitoring/http.rs"]
pub(crate) mod http;
#[path = "monitoring/lifecycle.rs"]
pub(crate) mod lifecycle;
#[path = "monitoring/observe.rs"]
pub(crate) mod observe;
#[path = "monitoring/permissions.rs"]
pub(crate) mod permissions;
#[path = "monitoring/preflight.rs"]
pub(crate) mod preflight;
#[path = "monitoring/process.rs"]
pub(crate) mod process;
#[path = "monitoring/publish.rs"]
pub(crate) mod publish;
#[path = "monitoring/receipts.rs"]
pub(crate) mod receipts;
#[path = "monitoring/repository.rs"]
pub(crate) mod repository;
#[path = "monitoring/runtime.rs"]
pub(crate) mod runtime;
#[path = "monitoring/staging.rs"]
pub(crate) mod staging;
#[path = "monitoring/status.rs"]
pub(crate) mod status;
#[path = "monitoring/supervisor.rs"]
pub(crate) mod supervisor;
#[path = "monitoring/tools.rs"]
pub(crate) mod tools;
#[path = "monitoring/webhook.rs"]
pub(crate) mod webhook;

pub(super) fn run(command: &MonitoringCommand, root: &Path) -> Result<()> {
    if print_help(command) {
        return Ok(());
    }
    let value = match command {
        MonitoringCommand::Bind(options) => bind_summary(bind::run(options, root)?),
        MonitoringCommand::Lifecycle { operation, binding } => {
            let result = match operation {
                MonitoringOperation::Start => lifecycle::start(root, binding)?,
                MonitoringOperation::Observe => observe::observe(root, binding)?,
                MonitoringOperation::Close => lifecycle::close(root, binding)?,
                MonitoringOperation::Result => status::result(root, binding)?,
                MonitoringOperation::Status => status::status(root, binding)?,
            };
            if matches!(
                operation,
                MonitoringOperation::Status | MonitoringOperation::Result
            ) {
                result
            } else {
                lifecycle_summary(*operation, &result)
            }
        }
        MonitoringCommand::Help
        | MonitoringCommand::BindHelp
        | MonitoringCommand::LifecycleHelp(_) => unreachable!("帮助已在前面返回"),
    };
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

fn bind_summary(value: serde_json::Value) -> serde_json::Value {
    json!({
        "status": "bound",
        "run_id": value["run_id"],
        "scope_id": value["scope_id"],
        "binding": value["_binding"],
        "external_contacts": 0,
    })
}

fn lifecycle_summary(
    operation: MonitoringOperation,
    value: &serde_json::Value,
) -> serde_json::Value {
    json!({
        "status": value["status"].as_str().unwrap_or(match operation {
            MonitoringOperation::Start => "running",
            MonitoringOperation::Observe => "observed",
            MonitoringOperation::Close => "closed",
            MonitoringOperation::Result => "passed",
            MonitoringOperation::Status => "bound",
        }),
        "run_id": value["run_id"],
        "scope_id": value["scope_id"],
        "binding": value["binding"],
        "external_contacts": 0,
    })
}

fn print_help(command: &MonitoringCommand) -> bool {
    let text = match command {
        MonitoringCommand::Help => {
            "cargo xtask check recovery monitoring <bind|start|observe|close|result|status> ...\n\
             bind 与 start/observe/close/result 会写入阶段证据，必须显式传入 --write；status 只读。"
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
            println!(
                "监控进程、观测与关闭由 Rust xtask 执行；正式恢复来源及存活代次仍复用现有 Python 核验器"
            );
            return true;
        }
        MonitoringCommand::Bind(_) | MonitoringCommand::Lifecycle { .. } => return false,
    };
    println!("{text}");
    true
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    let command = arguments.next()?;
    let command = command.to_string_lossy();
    if !matches!(
        command.as_ref(),
        "__monitoring-supervisor" | "__monitoring-webhook"
    ) {
        return None;
    }
    let extra = arguments
        .next()
        .map(|value| value.to_string_lossy().into_owned());
    if arguments.next().is_some() {
        return Some(2);
    }
    let result = match command.as_ref() {
        "__monitoring-supervisor" if extra.is_none() => std::env::var(supervisor::ENVIRONMENT_KEY)
            .map_err(|_| "监控 supervisor 缺少私有协议".into())
            .and_then(|value| supervisor::Protocol::decode(&value))
            .and_then(|protocol| supervisor::run(&protocol)),
        "__monitoring-webhook" => extra
            .ok_or_else(|| "监控 webhook 缺少私有协议".into())
            .and_then(|value| webhook::Protocol::decode(&value))
            .and_then(|protocol| webhook::run(&protocol)),
        "__monitoring-supervisor" => Err("监控内部阶段参数无效".into()),
        _ => unreachable!("前序匹配保证只处理监控内部命令"),
    };
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("监控内部阶段失败：{error}");
            1
        }
    })
}
