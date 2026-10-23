//! 缓存恢复的请求、收据与有界控制调用；不接管外层锁或重放写入。
mod evidence;
mod facts;
mod model;
mod process;

use super::json as document;
use crate::Result;
use serde_json::{Value, json};
use std::io::{Read, Write};

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) != Some("__cache-process") {
        return None;
    }
    let result = (|| {
        if args.len() != 1 {
            return Err("缓存控制只接受有界标准输入".into());
        }
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        execute(&crate::workspace::root_dir(), &bytes)
    })();
    let (code, response) = match result {
        Ok(value) => (0, json!({"ok":true,"result":value})),
        // 解析错误和远端输出可能包含密码；只发布固定诊断，禁止重放。
        Err(error) => {
            let category = match error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind)
            {
                Some(std::io::ErrorKind::ConnectionRefused) => "CacheConnectionRefused",
                Some(std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
                    "CacheObservationTimeout"
                }
                _ if error.downcast_ref::<process::ControlFailure>().is_some() => {
                    "CacheControlFailed"
                }
                _ => "CacheProcessRejected",
            };
            (
                1,
                json!({"ok":false,"error":category,"exit_code":error.downcast_ref::<process::ControlFailure>().and_then(|failure| failure.0),
                "message":"缓存请求、冻结收据或 WSL 精确进程调用失败；保留意图、前后像与锁，核对绑定配置、工具摘要及进程代次后显式恢复，禁止自动重放。"}),
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

pub(crate) fn execute(backend: &std::path::Path, bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() || bytes.len() > 1024 * 1024 {
        return Err("缓存控制输入为空或超过1MiB".into());
    }
    let value = document::parse_fragment(std::str::from_utf8(bytes)?, "缓存控制")?;
    document::exact(
        &value,
        &["format_version", "kind", "operation", "arguments"],
        "缓存控制",
    )?;
    if value["format_version"].as_u64() != Some(1) || value["kind"] != "ryframe-cache-process" {
        return Err("缓存控制协议版本或类型无效".into());
    }
    let args = &value["arguments"];
    match value["operation"].as_str() {
        Some("validate") => {
            document::exact(args, &["request", "files"], "缓存请求校验")?;
            model::validate(
                backend,
                &args["request"],
                document::boolean(&args["files"], "files")?,
            )?;
            Ok(Value::Null)
        }
        Some("password") => {
            document::exact(args, &["value"], "缓存密码校验")?;
            model::password(&args["value"])?;
            Ok(Value::Null)
        }
        Some("inspect") => {
            document::exact(args, &["request", "output"], "缓存创建证据")?;
            evidence::inspect(
                &args["request"],
                std::path::Path::new(document::text(&args["output"], "缓存证据目录")?),
            )
        }
        Some("facts") => {
            document::exact(args, &["request"], "缓存只读核验")?;
            model::validate(backend, &args["request"], false)?;
            facts::observe(&args["request"])
        }
        Some("check-facts") => {
            document::exact(
                args,
                &["request", "identity", "facts", "run_id"],
                "缓存代次核验",
            )?;
            facts::check(
                &args["request"],
                &args["identity"],
                &args["facts"],
                &args["run_id"],
            )?;
            Ok(Value::Null)
        }
        Some("call") => {
            document::exact(args, &["request", "runtime", "action"], "缓存进程调用")?;
            process::call(
                backend,
                &args["request"],
                &args["runtime"],
                document::text(&args["action"], "缓存控制动作")?,
            )
        }
        _ => Err("未知缓存控制操作".into()),
    }
}

#[cfg(all(test, windows))]
mod process_tests;
#[cfg(test)]
mod tests;
