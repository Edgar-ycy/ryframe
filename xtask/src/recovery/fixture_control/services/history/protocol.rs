use std::{env, io::Read, path::PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use super::{evidence, validate, validate_manifest};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const MARKER: &str = "__fixture-service-history";
const LIMIT: usize = 4 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    format_version: u8,
    operation: String,
    backend: PathBuf,
    arguments: Value,
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut args = env::args_os().skip(1);
    if args.next()?.as_os_str() != MARKER {
        return None;
    }
    let mut input = Vec::new();
    let read = std::io::stdin()
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut input);
    let (code, response) = if args.next().is_some() || read.is_err() {
        failure(None)
    } else {
        execute(&input)
    };
    println!("{response}");
    Some(code)
}

pub(crate) fn execute(input: &[u8]) -> (i32, Value) {
    match dispatch(input) {
        Ok(result) => {
            let response = json!({"ok":true,"result":result});
            if response.to_string().len() > LIMIT {
                failure(Some("夹具私有响应超过 4 MiB；缩小输入，禁止输出原始凭据"))
            } else {
                (0, response)
            }
        }
        Err(error) => {
            let reason = error
                .downcast_ref::<evidence::HistoryError>()
                .map(|error| error.0)
                .or_else(|| {
                    error
                        .downcast_ref::<super::super::lifecycle::model::LifecycleError>()
                        .map(|error| error.message)
                })
                .map(str::to_owned)
                .or_else(|| {
                    error
                        .downcast_ref::<crate::recovery::clone::native::cache::ObserverFailure>()
                        .map(ToString::to_string)
                });
            failure(reason.as_deref())
        }
    }
}

fn failure(reason: Option<&str>) -> (i32, Value) {
    (
        2,
        json!({"ok":false,"error":{"code":"invalid_fixture_history",
        "message":reason.unwrap_or("夹具历史校验失败；检查绑定文件、完整账本及证据格式，保留现场，禁止自动重放")}}),
    )
}

fn dispatch(input: &[u8]) -> Result<Value> {
    evidence::check(
        !input.is_empty() && input.len() <= LIMIT,
        "夹具历史请求为空或超过 4 MiB",
    )?;
    let text = std::str::from_utf8(input)?;
    crate::recovery::clone::native::json::parse_fragment(text, "夹具历史请求")?;
    let request: Request = serde_json::from_str(text)?;
    evidence::check(request.format_version == 1, "夹具历史协议版本无效")?;
    let args = &request.arguments;
    match request.operation.as_str() {
        "manifest" => {
            evidence::exact(args, &["value"])?;
            validate_manifest(&args["value"])?;
            Ok(Value::Null)
        }
        "initial" => {
            evidence::exact(args, &["state"])?;
            let attempts =
                crate::recovery::clone::native::lifecycle::state::attempts(&args["state"])?;
            evidence::check(
                attempts.len() >= super::INITIAL.len()
                    && attempts
                        .iter()
                        .zip(super::INITIAL)
                        .all(|(item, (stage, mode))| {
                            item["stage"] == stage
                                && item["mode"] == mode
                                && item["status"] == "passed"
                        }),
                "夹具服务账本必须完整包含 RustFS、Redis 与对象桶初始化",
            )?;
            Ok(Value::Null)
        }
        "history" => {
            evidence::exact(args, &["run", "state", "ready", "settled"])?;
            let run = run_path(&request.backend, args)?;
            let ready = args["ready"].as_bool().ok_or("ready 必须为布尔值")?;
            let settled = args["settled"].as_bool().ok_or("settled 必须为布尔值")?;
            validate(&run, &args["state"], ready, settled)
        }
        operation if operation.starts_with("service-") => {
            super::super::lifecycle::query::dispatch(&request.backend, operation, args)
        }
        _ => Err("未知夹具历史操作".into()),
    }
}

fn run_path(backend: &std::path::Path, args: &Value) -> Result<PathBuf> {
    let path = PathBuf::from(evidence::text(args, "run")?);
    validate_local_test_path(&path, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(Into::into)
}
