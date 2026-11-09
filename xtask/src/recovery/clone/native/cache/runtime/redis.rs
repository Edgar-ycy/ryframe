//! 现有 RESP 客户端维持字符串整数契约；仅缓存响应在本地恢复原收据数值。
use super::Native;
#[cfg(windows)]
use crate::recovery::clone::native::target_verify::redis::Client;
use crate::{Result, recovery::clone::native::json as document};
use serde_json::Value;
#[cfg(windows)]
use serde_json::json;

pub(super) fn command(runtime: &mut Native<'_>, args: &Value) -> Result<Value> {
    document::exact(args, &["request", "guard", "parts"], "缓存 Redis 端口")?;
    let guard = &args["guard"];
    let request = &args["request"];
    if guard["request"] != *request {
        return Err("缓存 Redis 端口必须使用同一已核验的固定请求".into());
    }
    super::super::require_windows(&guard["process"])?;
    let parts = document::array(&args["parts"], "缓存 Redis 命令")?
        .iter()
        .map(|item| document::text(item, "缓存 Redis 命令字段"))
        .collect::<Result<Vec<_>>>()?;
    let port = request["previous"]["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .ok_or("缓存 Redis 端口无效")?;
    let password = document::text(&guard["process"]["password"], "缓存绑定认证")?;
    runtime.dispatch("guard", guard.clone())?;
    #[cfg(windows)]
    {
        let response = windows_command(runtime, guard, port, password, &parts)?;
        if parts.first() == Some(&"MSETNX") {
            return match response.as_str() {
                Some("1") => Ok(json!(1)),
                Some("0") => Ok(json!(0)),
                _ => Err("缓存 MSETNX 响应未确认；保留意图和完整前后像，禁止重放".into()),
            };
        }
        Ok(response)
    }
    #[cfg(not(windows))]
    {
        let _ = (port, password, parts);
        Err("Windows Redis 缓存写入只能在本机 Windows 核验".into())
    }
}

#[cfg(windows)]
fn windows_command(
    runtime: &mut Native<'_>,
    guard: &Value,
    port: u16,
    password: &str,
    parts: &[&str],
) -> Result<Value> {
    let active = &guard["runtime"];
    let redis = &active["redis"];
    if active["format_version"] != 3
        || redis["platform"] != "windows"
        || redis["port"] != port
        || redis["tool"] != guard["process"]["tool"]
    {
        return Err("Windows Redis 缓存命令缺少原登记运行代次".into());
    }
    let mut client = Client::authenticated_windows(port, redis, password)?;
    runtime.dispatch("guard", guard.clone())?;
    client.recheck_windows_peer()?;
    let response = client.command(parts);
    let response = combine(response, client.recheck_windows_peer());
    combine(
        response,
        runtime.dispatch("guard", guard.clone()).map(|_| ()),
    )
}

#[cfg(windows)]
fn combine<T>(result: Result<T>, after: Result<()>) -> Result<T> {
    match (result, after) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(after)) => Err(format!("{error}；缓存命令后验失败：{after}").into()),
    }
}
