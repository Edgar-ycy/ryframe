//! CI 四库准备的私有协议；请求经管道传递，错误不包含客户端输出或配置值。

#[path = "databases/client.rs"]
mod client;
#[path = "databases/files.rs"]
pub(crate) mod files;
#[path = "databases/model.rs"]
mod model;
#[path = "databases/prepare.rs"]
mod prepare;
#[cfg(test)]
#[path = "databases/tests.rs"]
mod tests;

#[path = "databases/snapshot.rs"]
pub(crate) mod snapshot;

use crate::devex::runtime_native::process::Cancellation;
use serde_json::{Value, json};
use std::io::Read;

type Checked<T> = Result<T, &'static str>;
const LIMIT: usize = 4 * 1024 * 1024;
const MARKER: &str = "__ci-databases";

pub(crate) fn load_control(root: &std::path::Path) -> crate::Result<Value> {
    model::control(root).map_err(|code| {
        format!("全栈控制文件无效：{code}；检查 .local-tests/full-stack-control.json").into()
    })
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_none_or(|v| v != MARKER) {
        return None;
    }
    let mut input = Vec::new();
    let read = std::io::stdin()
        .take(LIMIT as u64 + 1)
        .read_to_end(&mut input);
    let result = if args.len() != 2 || read.is_err() || input.len() > LIMIT {
        Err("protocol_request")
    } else {
        dispatch(&args[1].to_string_lossy(), &input)
    };
    let (code, output) = response(result);
    println!("{output}");
    Some(code)
}

fn dispatch(operation: &str, input: &[u8]) -> Checked<Value> {
    if input.len() > LIMIT {
        return Err("protocol_request");
    }
    let value = crate::recovery::clone::native::json::parse_fragment(
        std::str::from_utf8(input).map_err(|_| "protocol_request")?,
        "CI数据库协议",
    )
    .map_err(|_| "protocol_request")?;
    match operation {
        "private-directory" | "instance-text" => files::dispatch(operation, value),
        "control" => {
            let request: model::ControlRequest =
                serde_json::from_value(value).map_err(|_| "protocol_request")?;
            model::control(&request.backend)
        }
        "plan" | "prepare" => {
            let request: model::Input =
                serde_json::from_value(value).map_err(|_| "protocol_request")?;
            let plan = request.validate()?;
            if operation == "plan" {
                Ok(plan.public())
            } else {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "runtime_initialization")?;
                runtime.block_on(async {
                    let cancel = Cancellation::default();
                    let trigger = cancel.clone();
                    let signal = tokio::spawn(async move {
                        interrupted().await;
                        trigger.cancel();
                    });
                    let result = prepare::execute(&plan, &cancel).await;
                    signal.abort();
                    result
                })
            }
        }
        _ => Err("protocol_operation"),
    }
}

async fn interrupted() {
    #[cfg(windows)]
    if let Ok(mut signal) = tokio::signal::windows::ctrl_break() {
        let ctrl_c = tokio::signal::ctrl_c();
        let ctrl_break = signal.recv();
        futures_util::pin_mut!(ctrl_c, ctrl_break);
        futures_util::future::select(ctrl_c, ctrl_break).await;
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn response(result: Checked<Value>) -> (i32, Value) {
    match result {
        Ok(result) => (0, json!({"ok":true,"result":result})),
        Err(code) => {
            let message = match code {
                "creation_unknown" => {
                    "隔离 MySQL 建库失败；创建结果未知。保留 database-targets.json，核对四库前后像，禁止自动重放。"
                }
                "replay_refused" => {
                    "运行目录已有数据库准备记录，拒绝自动重放；请核对登记资源和已有收据。"
                }
                "instance_id" => {
                    "snowflake_worker_id 必须为 0–1023 的整数且不能与 Worker 固定编号 2 冲突。"
                }
                "control_invalid" => {
                    "检查 .local-tests/full-stack-control.json：要求普通文件、版本 1、绝对路径和明确的容器 ID 或客户端路径。"
                }
                "binding_invalid" => {
                    "检查 database.primary：要求当前 CI 的本机地址、端口、库名、明文凭据及完整 TLS 配置；禁止环境引用。"
                }
                "config_publish" => {
                    "数据库已创建但配置发布未完成；保留收据和配置目录，核对现场，禁止自动重放。"
                }
                _ => {
                    "CI 数据库准备未完成；检查显式配置目录、当前 Job 标识、文件权限及协议版本，保留现场。"
                }
            };
            (
                1,
                json!({"ok":false,"error":{"code":code,"message":message}}),
            )
        }
    }
}
