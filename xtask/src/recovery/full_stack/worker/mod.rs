//! 外部 Worker 的原生控制、独立监督与有界收据协议。
pub(crate) mod controller;
mod documents;
mod environment;
mod finite;
mod guard;
mod launch;
mod operation;
pub(in crate::recovery) mod preparation;
mod probes;
mod process;
mod receipts;
pub(crate) mod role;
mod runtime;
pub(crate) mod start;
mod supervisor;
pub(in crate::recovery) use supervisor::{read_stop_request, supervise_product};
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/full_stack/worker/tests.rs"]
mod tests;
mod tree;

use crate::{Result, recovery::clone::native::json as strict};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

pub(crate) fn control_command(
    role: role::Role,
    backend: &std::path::Path,
    directory: &std::path::Path,
    action: &str,
    timeout: f64,
) -> Result<std::process::Command> {
    if role == role::Role::Frontend {
        return Err("前端有限命令不能使用长期服务控制入口".into());
    }
    let mut args =
        json!({"backend":backend,"directory":directory,"action":action,"timeout":timeout});
    let operation = if role == role::Role::Worker {
        "control"
    } else {
        args["role"] = json!(role.name());
        "role-control"
    };
    launch::command(operation, &args, directory)
}

pub(super) fn wait_api_port_free(url: &str) -> Result<Value> {
    probes::execute(&json!({"action":"wait-free","url":url,"timeout":5}))?;
    Ok(json!({"state":"free"}))
}

pub(super) fn ci_api_evidence(directory: &Path) -> Result<bool> {
    use crate::local_test_path::{LocalTestPathKind, validate_absolute_path};

    validate_absolute_path(directory, LocalTestPathKind::ExistingDirectory)?;
    let mut product = false;
    let mut tree = false;
    let mut archive = false;
    let mut generation = None;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let lower = name.to_ascii_lowercase();
        if lower != "api.json" && lower != "api.log" && !lower.starts_with("api-") {
            continue;
        }
        let kind = match name.as_ref() {
            "api.json" => {
                product = true;
                LocalTestPathKind::ExistingFile
            }
            "api-tree.json" => {
                tree = true;
                LocalTestPathKind::ExistingFile
            }
            "api-control.lock" => return Err("API 尚有原控制锁；保留现场，禁止自动重放".into()),
            "api-control-receipts" => {
                archive = true;
                LocalTestPathKind::ExistingDirectory
            }
            "api.log" | "api-control.guard" => LocalTestPathKind::ExistingFile,
            name => {
                let id = name
                    .strip_prefix("api-members-")
                    .and_then(|v| {
                        v.strip_suffix("-ready.json")
                            .or_else(|| v.strip_suffix("-stopped.json"))
                    })
                    .or_else(|| {
                        let tree = name.strip_prefix("api-tree-")?;
                        tree.strip_suffix("-control.json")
                            .or_else(|| tree.strip_suffix("-result.json"))
                    })
                    .or_else(|| {
                        name.strip_prefix("api-external-termination-")?
                            .strip_suffix(".json")
                    });
                if let Some(id) = id {
                    if !operation_id(id)
                        || generation
                            .replace(id.to_owned())
                            .is_some_and(|prior| prior != id)
                    {
                        return Err("API 成员收据未绑定同一启动代次；保留现场".into());
                    }
                } else if !name
                    .strip_prefix("api-")
                    .and_then(|v| v.strip_suffix(".log"))
                    .is_some_and(operation_id)
                {
                    return Err("发现未知 API 文件；保留现场，禁止自动重放".into());
                }
                LocalTestPathKind::ExistingFile
            }
        };
        validate_absolute_path(&entry.path(), kind)?;
    }
    if product && tree {
        if let Some(id) = generation
            && operation::read(&directory.join("api-tree.json"))?["operation_id"] != id
        {
            return Err("API 成员收据未绑定顶层进程树；保留现场".into());
        }
        return Ok(true);
    }
    if product || tree || archive || generation.is_some() {
        return Err("API 进程与成员证据不完整；保留现场，禁止自动重放".into());
    }
    Ok(false)
}

fn operation_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn ci_api_action(role: role::Role, action: &str) -> Result<(&str, bool)> {
    match action {
        "start-strict" if role == role::Role::Api => Ok(("start", true)),
        "stop-strict" if role == role::Role::Api => Ok(("stop", true)),
        "start" | "stop" | "crash" | "status" | "reconcile" => Ok((action, false)),
        _ => reject("角色控制操作无效"),
    }
}

const LIMIT: usize = 1024 * 1024;
pub(in crate::recovery) const CONTROL_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(300);

fn check_result_deadline(role: role::Role, deadline: std::time::Instant) -> Result<()> {
    if std::time::Instant::now() >= deadline {
        return Err(Rejection(format!(
            "{}后台启动结果未知；使用status或reconcile核对，不自动重放",
            role.label()
        ))
        .into());
    }
    Ok(())
}
// 候选准备包含完整来源核验；不占用产品启动后的就绪预算。
fn preparation_timeout(role: role::Role) -> std::time::Duration {
    std::time::Duration::from_secs(if role == role::Role::Frontend {
        10
    } else {
        120
    })
}

fn check_preparation(role: role::Role, deadline: std::time::Instant) -> Result<()> {
    if std::time::Instant::now() >= deadline {
        return Err(Rejection(format!(
            "{}监督候选登记超过{}秒期限",
            role.label(),
            preparation_timeout(role).as_secs()
        ))
        .into());
    }
    Ok(())
}

pub(in crate::recovery) fn frontend_evidence(
    directory: &std::path::Path,
    scope: &str,
    terminate: bool,
) -> Result<Value> {
    let recorded = operation::read(&directory.join("runtime.json"))?;
    let backend = std::path::Path::new(
        recorded["coordinator"]
            .as_str()
            .ok_or("前端命令缺少协调器")?,
    );
    let runtime = runtime::load(role::Role::Frontend, backend, directory)?;
    if runtime["scope_id"] != scope {
        return Err("前端进程 scope 与冻结输入不同".into());
    }
    process::registered(role::Role::Frontend, directory, &runtime)?;
    let tree =
        tree::read(directory, &runtime, role::Role::Frontend, true)?.ok_or("前端进程树缺失")?;
    if terminate {
        tree::stop(&tree, true)?;
    }
    let proof = tree::closure(&tree, std::time::Duration::from_secs(10), false)?;
    let path = tree::path(
        directory,
        role::Role::Frontend,
        tree["operation_id"].as_str().ok_or("缺少操作 ID")?,
        "stopped",
    );
    runtime::load(role::Role::Frontend, backend, directory)?;
    let completion = operation::file(&path)?;
    Ok(
        json!({"tree":tree,"completion":{"path":path,"bytes":completion["bytes"],"sha256":completion["sha256"]},"proof":proof}),
    )
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !matches!(
        args.first().map(String::as_str),
        Some(
            "__full-stack-worker"
                | "__full-stack-worker-detached"
                | "__full-stack-process"
                | "__full-stack-process-detached"
        )
    ) {
        return None;
    }
    let worker = args[0].starts_with("__full-stack-worker");
    let kind = if worker {
        "ryframe-full-stack-worker"
    } else {
        "ryframe-full-stack-process"
    };
    let result = (|| {
        if args[0].ends_with("-detached") {
            if args.len() != 2 || args[1].len() > 32768 {
                return Err("Worker monitor 参数无效".into());
            }
            let value = strict::parse_fragment(&args[1], "Worker monitor 参数")?;
            if value["operation"] != "monitor" || value["kind"] != kind {
                return Err("Worker detached 只允许 monitor".into());
            }
            return execute(args[1].as_bytes());
        }
        if args.len() != 1 {
            return Err("Worker 原生守卫只接受有界标准输入".into());
        }
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        let value = strict::parse_fragment(std::str::from_utf8(&bytes)?, "监督请求")?;
        if value["kind"] != kind {
            return Err("监督入口与请求类型不一致".into());
        }
        execute(&bytes)
    })();
    let (code, response) = match result {
        Ok(value) => (0, json!({"ok":true,"result":value})),
        Err(error) => {
            // 只返回固定的守卫错误，不转发输入、URL、收据或底层 I/O 内容。
            let message = error
                .downcast_ref::<Rejection>()
                .map(|e| e.0.as_str())
                .unwrap_or("受监督进程原生核验失败；保留收据与锁，核对正式工具和输入后显式恢复");
            (
                1,
                json!({"ok":false,"error":if worker {"WorkerRejected"} else {"ProcessRejected"},"message":message}),
            )
        }
    };
    Some(
        if writeln!(std::io::stdout().lock(), "{response}").is_ok() {
            code
        } else {
            1
        },
    )
}

#[derive(Debug)]
struct Rejection(String);
impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Rejection {}
fn reject<T>(message: &'static str) -> Result<T> {
    Err(Rejection(message.into()).into())
}

pub(crate) fn execute(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() || bytes.len() > LIMIT {
        return reject("Worker 原生请求为空或超过 1 MiB");
    }
    let request = strict::parse_fragment(std::str::from_utf8(bytes)?, "Worker 原生请求")?;
    strict::exact(
        &request,
        &["format_version", "kind", "operation", "arguments"],
        "Worker 原生请求",
    )?;
    if request["format_version"].as_u64() != Some(1)
        || !matches!(
            request["kind"].as_str(),
            Some("ryframe-full-stack-worker" | "ryframe-full-stack-process")
        )
    {
        return reject("Worker 原生请求类型或版本无效");
    }
    let worker = request["kind"] == "ryframe-full-stack-worker";
    let operation = request["operation"].as_str();
    if matches!(operation, Some("monitor" | "supervise" | "detach")) {
        let role = role::Role::parse(&request["arguments"]["role"])?;
        if (role == role::Role::Worker) != worker {
            return reject("监督角色与协议类型不一致");
        }
    } else if worker != matches!(operation, Some("probe" | "validate" | "control")) {
        return reject("角色协议与控制操作不一致");
    }
    match request["operation"].as_str() {
        Some("probe") => probes::execute(&request["arguments"]),
        Some("validate") => receipts::execute(&request["arguments"]),
        Some("control") => controller::execute(role::Role::Worker, &request["arguments"]),
        Some("role-control") => {
            let mut args = request["arguments"].clone();
            let role = role::Role::parse(&args["role"])?;
            if role != role::Role::Api {
                return reject(
                    "role-control 仅允许 API；Worker 使用 control，前端使用 run-frontend",
                );
            }
            args.as_object_mut()
                .ok_or("角色参数必须为对象")?
                .remove("role");
            controller::execute(role, &args)
        }
        Some("run-frontend") => finite::execute(&request["arguments"]),
        Some("monitor") => supervisor::monitor(&request["arguments"]),
        Some("supervise") => supervisor::run(&request["arguments"]),
        #[cfg(windows)]
        Some("detach") => launch::detach_platform(&request["arguments"]),
        _ => reject("未知 Worker 原生守卫操作"),
    }
}
