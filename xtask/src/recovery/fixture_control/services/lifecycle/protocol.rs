use super::model::{self, check};
use crate::Result;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{env, io::Read, path::PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    format_version: u8,
    backend: PathBuf,
    operation: String,
    review: PathBuf,
    environment: PathBuf,
    owner: Option<PathBuf>,
    write: bool,
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut args = env::args_os().skip(1);
    if args.next()?.as_os_str() != "__fixture-service-lifecycle" {
        return None;
    }
    let mut input = Vec::new();
    let read = std::io::stdin()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut input);
    let (code, response) = if read.is_err() || args.next().is_some() {
        failure(None)
    } else {
        execute(&input)
    };
    println!("{response}");
    Some(code)
}

pub(crate) fn execute(input: &[u8]) -> (i32, Value) {
    match dispatch(input) {
        Ok(value) => {
            let response = json!({"ok":true,"result":value});
            if response.to_string().len() > 4 * 1024 * 1024 {
                failure(Some("夹具生命周期响应超过 4 MiB；保留现场，不回显私有内容"))
            } else {
                (0, response)
            }
        }
        Err(error) => failure(
            error
                .downcast_ref::<model::LifecycleError>()
                .map(|error| error.message),
        ),
    }
}

fn failure(reason: Option<&str>) -> (i32, Value) {
    (
        2,
        json!({"ok":false,"error":{"code":"invalid_fixture_lifecycle",
        "message":reason.unwrap_or("夹具生命周期未完成；核对输入路径、来源、完整账本和原服务证据，保留现场，不自动重放")}}),
    )
}

fn dispatch(input: &[u8]) -> Result<Value> {
    check(
        !input.is_empty() && input.len() <= 4 * 1024 * 1024,
        "生命周期请求为空或超过 4 MiB",
    )?;
    let raw = std::str::from_utf8(input)?;
    crate::recovery::clone::native::json::parse_fragment(raw, "生命周期请求")?;
    let value: Request = serde_json::from_str(raw)?;
    check(value.format_version == 1, "生命周期协议版本无效")?;
    check(
        if value.operation == "status" {
            !value.write
        } else {
            value.write
        },
        "服务状态只读；生命周期操作必须显式指定 --write",
    )?;
    check(
        ["status", "close", "recover", "reconcile", "restart"].contains(&value.operation.as_str()),
        "未知生命周期操作",
    )?;
    check(
        ["recover", "reconcile", "restart"].contains(&value.operation.as_str())
            == value.owner.is_some(),
        "生命周期 owner 与操作不一致",
    )?;
    for path in [&value.review, &value.environment] {
        crate::local_test_path::validate_local_test_path(
            path,
            &value.backend,
            crate::local_test_path::LocalTestPathKind::ExistingFile,
        )?;
    }
    super::execute(
        &value.backend,
        &value.operation,
        &value.review,
        &value.environment,
        value.owner.as_deref(),
        value.write,
    )
}
