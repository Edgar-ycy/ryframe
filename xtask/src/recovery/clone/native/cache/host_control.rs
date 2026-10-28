//! Windows 端的原代次观察与停止；失败停止仅允许一次只读复核。
use super::{
    super::{json as doc, resources::common},
    evidence, facts, model, native_tool, process,
};
use crate::{
    Result, recovery::fixture_control::services::lifecycle::readonly::process as identity,
};
use serde_json::{Value, json};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
    time::{Duration, Instant},
};

pub(super) fn bind_and_close(request: &Value) -> Result<()> {
    let port = u16::try_from(request["port"].as_u64().ok_or("缓存端口无效")?)?;
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(false)?;
    socket.bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into())?;
    drop(socket);
    Ok(())
}

pub(super) fn launcher(request: &Value, runtime: &Value, required: bool) -> Result<bool> {
    let expected = &runtime["launcher"];
    model::identity(expected, false)?;
    let pid = u32::try_from(expected["pid"].as_u64().ok_or("WSL PID 无效")?)?;
    let Some(actual) = common::process_identity(pid)? else {
        if required {
            return Err("缓存 WSL 监督器已退出".into());
        }
        return Ok(false);
    };
    let output = Path::new(doc::text(&runtime["output"], "缓存运行目录")?);
    if actual != *expected
        || identity::arguments(expected)? != evidence::command(request, output, "serve")?
    {
        return Err("缓存 WSL 监督器创建身份或完整参数变化".into());
    }
    Ok(true)
}

pub(crate) fn observe(backend: &Path, request: &Value, runtime: &Value) -> Result<Value> {
    model::validate(backend, request, true)?;
    let tool = native_tool::from_request(request)?;
    tool.finish(backend)?;
    launcher(request, runtime, true)?;
    let before = process::call(backend, request, runtime, "observe")?;
    if before["alive"] != true || before["boot_id"] != runtime["boot_id"] {
        return Err("缓存原运行代次已退出".into());
    }
    let observed = facts::bounded(backend, request)?;
    facts::check(
        request,
        &runtime["linux_identity"],
        &observed,
        &runtime["redis"]["run_id"],
    )?;
    if process::call(backend, request, runtime, "observe")? != before {
        return Err("缓存认证观察期间内核代次变化".into());
    }
    let mut redis = json!({});
    for key in [
        "wsl",
        "distribution",
        "port",
        "executable",
        "sha256",
        "configuration",
    ] {
        redis[key] = request[key].clone();
    }
    redis["pid"] = runtime["linux_identity"]["pid"].clone();
    redis["started"] = runtime["linux_identity"]["started"].clone();
    redis["run_id"] = observed["run_id"].clone();
    let mut result = runtime.clone();
    result["redis"] = redis;
    result["facts"] = observed;
    tool.finish(backend)?;
    model::validate(backend, request, true)?;
    Ok(result)
}

pub(crate) fn stop(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    output: &Path,
) -> Result<Value> {
    model::validate(backend, request, true)?;
    common::local_directory(backend, output, "缓存停止输出")?;
    let tool = native_tool::from_request(request)?;
    tool.finish(backend)?;
    if runtime.get("partial_receipt").is_some() {
        let status = super::readonly::status(backend, request, runtime, None)?;
        if status["state"] != "stopped" {
            return Err("缓存部分启动未证明关闭".into());
        }
        let result = json!({"state":"stopped","status":"redis_process_stopped","runtime":runtime,"resources_deleted":false});
        super::linux_write::publish(&output.join("stopped.json"), &result)?;
        return Ok(result);
    }
    launcher(request, runtime, false)?;
    let (mut result, recovered) =
        stop_once(|action| process::call(backend, request, runtime, action))?;
    if result["alive"] != false {
        return Err("缓存停止后仍在运行".into());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while launcher(request, runtime, false)? {
        if Instant::now() >= deadline {
            return Err("Redis 关闭后 WSL 监督器未退出".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bind_and_close(request)?;
    tool.finish(backend)?;
    model::validate(backend, request, true)?;
    result["status"] = json!("redis_process_stopped");
    result["runtime"] = runtime.clone();
    result["resources_deleted"] = json!(false);
    if recovered {
        result["observed_after_failed_stop"] = json!(true);
    }
    super::linux_write::publish(&output.join("stopped.json"), &result)?;
    Ok(result)
}

pub(super) fn stop_once(mut call: impl FnMut(&str) -> Result<Value>) -> Result<(Value, bool)> {
    Ok(match call("stop") {
        Ok(value) => (value, false),
        Err(original) => match call("check") {
            Ok(value) if value["alive"] == false => (value, true),
            _ => return Err(original),
        },
    })
}
