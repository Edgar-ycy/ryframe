//! 缓存恢复的请求、收据与有界控制调用；不接管外层锁或重放写入。
pub(crate) mod binding;
mod cleanup_diagnostic;
mod diagnostic;
pub(crate) use cleanup_diagnostic::{
    decode as primitive_cleanup_error, detail as primitive_cleanup,
};
pub(crate) use diagnostic::{primitive_detail, primitive_error, primitive_message};
mod evidence;
mod facts;
pub(crate) mod failed_cleanup;
pub(crate) mod handoff;
mod history;
pub(crate) mod host_control;
pub(crate) mod host_start;
mod markers;
#[cfg(not(test))]
pub(in crate::recovery) mod model;
#[cfg(test)]
pub(crate) mod model;
mod native_tool;
pub(in crate::recovery) mod review_tools;
pub(crate) use native_tool::review_value as validate_native_tool;
pub(crate) use native_tool::successor as native_tool_successor;
mod process;
pub(crate) mod readonly;
pub(in crate::recovery) mod reboot;
mod request;
pub(crate) mod runner;
mod runtime;
mod session;
mod stage;
mod start_deadline;
#[cfg(windows)]
mod windows;
pub(in crate::recovery) mod windows_tool;

pub(crate) use runner::run;

use super::json as document;
use crate::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    #[cfg(windows)]
    if let Some(code) = windows::run_hidden_if_requested() {
        return Some(code);
    }
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) != Some("__cache-process") {
        return None;
    }
    let result = (|| {
        if args.len() != 1 {
            return Err("缓存控制只接受有界标准输入".into());
        }
        let mut bytes = Vec::new();
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        std::io::Read::take(&mut input, 1024 * 1024 + 1).read_until(b'\n', &mut bytes)?;
        if bytes.is_empty() || bytes.len() > 1024 * 1024 {
            return Err("缓存控制输入为空或超限".into());
        }
        let backend = crate::workspace::root_dir();
        let first = document::parse_fragment(std::str::from_utf8(&bytes)?, "缓存控制");
        if let Ok(value) = first
            && value["operation"] == "stage"
        {
            return session::execute(&backend, &value, &mut input, &mut std::io::stdout().lock());
        }
        let remaining = (1024 * 1024 + 1_usize).saturating_sub(bytes.len());
        std::io::Read::take(&mut input, remaining as u64).read_to_end(&mut bytes)?;
        execute(&backend, &bytes)
    })();
    let (code, response) = protocol_response(result);
    Some(
        if writeln!(std::io::stdout().lock(), "{response}").is_ok() {
            code
        } else {
            1
        },
    )
}

fn protocol_response(result: Result<Value>) -> (i32, Value) {
    match result {
        Ok(value) => (0, json!({"ok":true,"result":value})),
        // 解析错误和远端输出可能包含密码；只发布固定诊断，禁止重放。
        Err(error) => {
            let (_, exit_code) = primitive_detail(error.as_ref());
            let classified = cleanup_diagnostic::primary(error.as_ref());
            let category = match classified
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind)
            {
                Some(std::io::ErrorKind::ConnectionRefused) => "CacheConnectionRefused",
                Some(std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
                    "CacheObservationTimeout"
                }
                _ if classified
                    .downcast_ref::<process::ControlFailure>()
                    .is_some()
                    || classified
                        .downcast_ref::<process::RemoteFailure>()
                        .is_some()
                    || !exit_code.is_null() =>
                {
                    "CacheControlFailed"
                }
                _ => "CacheProcessRejected",
            };
            let detail = primitive_message(error.as_ref());
            (
                1,
                json!({"ok":false,"error":category,"exit_code":exit_code,
                "message":detail.unwrap_or_else(||policy_message(error.as_ref()).map_or_else(||"缓存请求、冻结收据或 Windows Redis 精确进程调用失败；保留意图、前后像与锁，核对绑定配置、工具摘要及进程代次后显式恢复，禁止自动重放。".to_string(),|reason|format!("{reason}。保留意图与完整前后像，核对固定请求和已登记代次后显式恢复，禁止自动重放。")))}),
            )
        }
    }
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
        Some("process-model") => request::bound_process(backend, args),
        Some("preflight-tools") => {
            document::exact(args, &["review"], "缓存原生工具预检")?;
            review_tools::observe(backend, &args["review"])
        }
        Some("observe") => {
            document::exact(args, &["request", "runtime"], "缓存原生观察")?;
            host_control::observe(backend, &args["request"], &args["runtime"])
        }
        Some("stop") => {
            document::exact(args, &["request", "runtime", "output"], "缓存原生停止")?;
            host_control::stop(
                backend,
                &args["request"],
                &args["runtime"],
                std::path::Path::new(document::text(&args["output"], "缓存停止输出")?),
            )
        }
        Some(
            operation @ ("environment-select" | "environment-model" | "request-head"
            | "request-model" | "successor-model" | "pending-target"),
        ) => request::run(operation, args),
        _ => execute_control(
            backend,
            value["operation"].as_str().ok_or("缓存操作必须为文本")?,
            args,
        ),
    }
}

fn execute_control(backend: &std::path::Path, operation: &str, args: &Value) -> Result<Value> {
    match operation {
        "images" => {
            document::exact(args, &["markers"], "缓存标记前后像")?;
            let (before, after) = markers::images(&args["markers"]);
            Ok(json!([before, after]))
        }
        "validate" => {
            document::exact(args, &["request", "files"], "缓存请求校验")?;
            model::validate(
                backend,
                &args["request"],
                document::boolean(&args["files"], "files")?,
            )?;
            Ok(Value::Null)
        }
        "password" => {
            document::exact(args, &["value"], "缓存密码校验")?;
            model::password(&args["value"])?;
            Ok(Value::Null)
        }
        "inspect" => {
            document::exact(args, &["request", "output"], "缓存创建证据")?;
            evidence::inspect(
                &args["request"],
                std::path::Path::new(document::text(&args["output"], "缓存证据目录")?),
            )
        }
        "facts" => {
            document::exact(args, &["request"], "缓存只读核验")?;
            Err(retired_wsl())
        }
        #[cfg(windows)]
        "windows-facts" => {
            document::exact(args, &["request", "identity"], "Windows 缓存只读核验")?;
            model::validate(backend, &args["request"], true)?;
            facts::observe_windows(&args["request"], &args["identity"])
        }
        "check-facts" => {
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
        "call" => {
            document::exact(args, &["request", "runtime", "action"], "缓存进程调用")?;
            Err(retired_wsl())
        }
        _ => Err("未知缓存控制操作".into()),
    }
}

#[cfg(all(test, windows))]
#[path = "../../../../../tests/unit/recovery/clone/native/cache/process_tests.rs"]
mod process_tests;
#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/cache/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/cache/stage_tests.rs"]
mod stage_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/cache/native_lifecycle.rs"]
mod native_lifecycle_tests;

#[derive(Debug)]
struct PolicyFailure(&'static str);
impl std::fmt::Display for PolicyFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}
impl std::error::Error for PolicyFailure {}
fn rejected(message: &'static str) -> Box<dyn std::error::Error> {
    Box::new(PolicyFailure(message))
}

fn require_windows(request: &Value) -> Result<()> {
    match request["format_version"].as_u64() {
        Some(3) => Ok(()),
        Some(1 | 2) => Err(retired_wsl()),
        None if request.get("format_version").is_none() && model::shape(request).is_ok() => {
            Err(retired_wsl())
        }
        _ => Err(rejected("缓存活体操作需要已登记的 Windows Redis v3 请求")),
    }
}

fn retired_wsl() -> Box<dyn std::error::Error> {
    rejected("历史 WSL Redis 代次仅供证据核验，请新建已登记 Windows Redis 的隔离代次")
}

pub(crate) fn policy_message(error: &(dyn std::error::Error + 'static)) -> Option<&'static str> {
    error
        .downcast_ref::<PolicyFailure>()
        .map(|failure| failure.0)
}
