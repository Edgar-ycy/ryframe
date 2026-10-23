//! 外部 Worker 的原生控制、独立监督与有界收据协议。
mod controller;
mod documents;
mod guard;
mod launch;
mod operation;
mod probes;
mod process;
mod receipts;
mod supervisor;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/full_stack/worker/tests.rs"]
mod tests;
mod tree;

use crate::{Result, recovery::clone::native::json as strict};
use serde_json::{Value, json};
use std::io::{Read, Write};

pub(crate) fn control_command(
    backend: &std::path::Path,
    directory: &std::path::Path,
    action: &str,
    timeout: f64,
) -> Result<std::process::Command> {
    launch::command(
        "control",
        &json!({"backend":backend,"directory":directory,"action":action,"timeout":timeout}),
        directory,
    )
}

const LIMIT: usize = 1024 * 1024;

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !matches!(
        args.first().map(String::as_str),
        Some("__full-stack-worker" | "__full-stack-worker-detached")
    ) {
        return None;
    }
    let result = (|| {
        if args[0] == "__full-stack-worker-detached" {
            if args.len() != 2 || args[1].len() > 32768 {
                return Err("Worker monitor 参数无效".into());
            }
            let value = strict::parse_fragment(&args[1], "Worker monitor 参数")?;
            if value["operation"] != "monitor" {
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
        execute(&bytes)
    })();
    let (code, response) = match result {
        Ok(value) => (0, json!({"ok":true,"result":value})),
        Err(error) => {
            // 只返回固定的守卫错误，不转发输入、URL、收据或底层 I/O 内容。
            let message = error
                .downcast_ref::<Rejection>()
                .map(|e| e.0)
                .unwrap_or("Worker 原生核验失败；保留收据与锁，核对正式工具和输入后显式恢复");
            (
                1,
                json!({"ok":false,"error":"WorkerRejected","message":message}),
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
struct Rejection(&'static str);
impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Rejection {}
fn reject<T>(message: &'static str) -> Result<T> {
    Err(Rejection(message).into())
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
        || request["kind"] != "ryframe-full-stack-worker"
    {
        return reject("Worker 原生请求类型或版本无效");
    }
    match request["operation"].as_str() {
        Some("probe") => probes::execute(&request["arguments"]),
        Some("validate") => receipts::execute(&request["arguments"]),
        Some("control") => controller::execute(&request["arguments"]),
        Some("monitor") => supervisor::monitor(&request["arguments"]),
        Some("supervise") => supervisor::run(&request["arguments"]),
        #[cfg(windows)]
        Some("detach") => launch::detach_platform(&request["arguments"]),
        _ => reject("未知 Worker 原生守卫操作"),
    }
}
