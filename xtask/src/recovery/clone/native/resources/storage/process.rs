use std::{
    path::Path,
    process::Child,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::super::{super::json as json_io, common};
use crate::Result;

pub(crate) fn running(expected: &Value, request: &Value, listeners: bool) -> Result<bool> {
    let expected = common::validate_process_identity(expected)?;
    let pid = u32::try_from(expected["pid"].as_u64().unwrap()).map_err(|_| "存储 PID 超出范围")?;
    let Some(current) = common::process_identity(pid)? else {
        return Ok(false);
    };
    if current != expected {
        return Err("存储 PID 已被复用，拒绝操作其他内核代次".into());
    }
    if listeners {
        for key in ["api_url", "console_url"] {
            let url = request
                .get(key)
                .and_then(Value::as_str)
                .ok_or("存储请求缺少监听端点")?;
            if !common::port_ready(url, Duration::from_millis(300)) {
                return Err("存储登记进程未监听已登记端口".into());
            }
        }
    }
    if common::process_identity(pid)?.as_ref() != Some(&expected) {
        return Err("存储监听核对期间内核代次变化".into());
    }
    Ok(true)
}

pub(crate) fn inspect_attempt(
    backend: &Path,
    output: &Path,
    registration: &Value,
    controller: &Value,
    number: u64,
) -> Result<Option<Value>> {
    let intent_path = output.join("intent.json");
    if !intent_path.exists() {
        if ["process.json", "launch.json", "ready.json", "spawned.json"]
            .iter()
            .any(|name| output.join(name).exists())
        {
            return Err("存储进程证据缺少启动意图，拒绝猜测来源".into());
        }
        return Ok(None);
    }
    let intent = json_io::read(&intent_path)?;
    let object = json_io::exact(
        &intent,
        &[
            "format_version",
            "kind",
            "request",
            "controller",
            "attempt",
            "arguments",
        ],
        "存储启动意图",
    )?;
    let request = common::read_bound(backend, registration, "存储固定请求")?;
    if object.get("format_version").and_then(Value::as_u64) != Some(1)
        || object.get("kind").and_then(Value::as_str) != Some("devex-clone-storage-intent")
        || object.get("request") != Some(registration)
        || object.get("controller") != Some(controller)
        || object.get("attempt").and_then(Value::as_u64) != Some(number)
        || object.get("arguments")
            != Some(&Value::Array(
                super::model::arguments(&request)?
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ))
    {
        return Err("存储意图不属于原注册与对应控制阶段".into());
    }
    common::bound_file(backend, controller, "存储控制器")?;
    if let Some(value) = not_started_state(output, &intent_path)? {
        return Ok(Some(value));
    }
    let receipt_path = output.join("process.json");
    let receipt = json_io::read(&receipt_path)?;
    let receipt = json_io::exact(
        &receipt,
        &[
            "format_version",
            "role",
            "scope_id",
            "lifecycle",
            "identity",
            "api_url",
            "console_url",
            "data_dir",
            "intent",
        ],
        "存储进程收据",
    )?;
    let identity = common::validate_process_identity(receipt.get("identity").unwrap())?;
    let stable = json!({
        "format_version": 1,
        "role": "rustfs",
        "scope_id": request["scope_id"],
        "lifecycle": "running",
        "api_url": request["api_url"],
        "console_url": request["console_url"],
        "data_dir": request["data_directory"]["path"],
        "intent": common::binding(&intent_path)?,
    });
    for (key, value) in stable.as_object().unwrap() {
        if receipt.get(key) != Some(value) {
            return Err("存储进程收据不属于该启动意图或实际工具".into());
        }
    }
    if identity["executable"].as_str()
        != request.pointer("/executable/path").and_then(Value::as_str)
    {
        return Err("存储进程可执行文件与固定请求不同".into());
    }
    let launch_path = output.join("launch.json");
    if launch_path.exists() {
        let launch = json_io::read(&launch_path)?;
        let expected = json!({
            "format_version": 1,
            "scope_id": request["scope_id"],
            "identity": identity,
            "arguments": super::model::arguments(&request)?,
            "credential_files": request["credential_files"],
        });
        if launch != expected {
            return Err("存储启动参数收据变化".into());
        }
    }
    Ok(Some(json!({
        "state": "recorded",
        "identity": identity,
        "process_receipt": common::binding(&receipt_path)?,
        "launch_receipt": launch_path.exists().then(|| common::binding(&launch_path)).transpose()?,
        "intent": common::binding(&intent_path)?,
    })))
}

fn not_started_state(output: &Path, intent_path: &Path) -> Result<Option<Value>> {
    if output.join("process.json").exists() {
        return Ok(None);
    }
    let intent = common::binding(intent_path)?;
    let not_started = output.join("not-started.json");
    if not_started.exists()
        && json_io::read(&not_started)?
            == json!({
                "intent": intent,
                "popen_called": false,
            })
    {
        return Ok(Some(json!({
            "state": "not_started",
            "intent": common::binding(intent_path)?,
        })));
    }
    let cleanup_path = output.join("failure-cleanup.json");
    if cleanup_path.exists() {
        let cleanup = json_io::read(&cleanup_path)?;
        let cleanup = json_io::exact(
            &cleanup,
            &["intent", "identity", "pid", "returncode"],
            "存储失败回收",
        )?;
        let pid = cleanup
            .get("pid")
            .and_then(Value::as_u64)
            .filter(|pid| *pid > 1)
            .ok_or("存储失败回收 PID 无效")?;
        let pid = u32::try_from(pid).map_err(|_| "存储失败回收 PID 超出范围")?;
        if cleanup.get("intent") != Some(&common::binding(intent_path)?)
            || cleanup.get("returncode").and_then(Value::as_i64).is_none()
            || cleanup
                .get("identity")
                .filter(|value| !value.is_null())
                .map(common::validate_process_identity)
                .transpose()?
                .is_some_and(|identity| identity["pid"].as_u64() != Some(u64::from(pid)))
            || common::process_identity(pid)?.is_some()
        {
            return Err("启动失败回收证据不完整或退出进程 PID 已被复用".into());
        }
        return Ok(Some(json!({
            "state": "not_started",
            "intent": common::binding(intent_path)?,
            "cleanup": common::binding(&cleanup_path)?,
        })));
    }
    Err("存储启动意图缺少完整创建身份；必须核实现场".into())
}

pub(crate) fn stop_record(request: &Value, observed: &Value, output: &Path) -> Result<Value> {
    if observed.get("state").and_then(Value::as_str) == Some("not_started") {
        let result = json!({
            "state": "stopped",
            "identity": null,
            "terminated": false,
            "process_receipt": null,
        });
        common::write_new(&output.join("stopped.json"), &result)?;
        return Ok(result);
    }
    let expected = observed.get("identity").ok_or("存储观察缺少身份")?;
    let alive = running(expected, request, false)?;
    let terminated = common::terminate_tree(expected)?;
    if common::process_identity(
        u32::try_from(expected["pid"].as_u64().unwrap()).map_err(|_| "存储 PID 超出范围")?,
    )?
    .is_some()
    {
        return Err("存储回收后仍存在进程或 PID 已被复用".into());
    }
    let result = json!({
        "state": "stopped",
        "identity": expected,
        "terminated": terminated && alive,
        "process_receipt": observed.get("process_receipt").cloned().unwrap_or(Value::Null),
    });
    common::write_new(&output.join("stopped.json"), &result)?;
    Ok(result)
}

pub(crate) fn start(
    request: &Value,
    environment: &std::collections::BTreeMap<String, String>,
    output: &Path,
    registration: &Value,
    controller: &Value,
    number: u64,
    mut guard: impl FnMut() -> Result<()>,
) -> Result<Value> {
    let args = super::model::arguments(request)?;
    let intent_path = output.join("intent.json");
    common::write_new(
        &intent_path,
        &json!({
            "format_version": 1,
            "kind": "devex-clone-storage-intent",
            "request": registration,
            "controller": controller,
            "attempt": number,
            "arguments": args,
        }),
    )?;
    let executable = request
        .pointer("/executable/path")
        .and_then(Value::as_str)
        .ok_or("存储请求缺少可执行文件")?;
    let mut command = common::command_line(executable, &args, environment, output)?;
    super::model::clear_overrides(&mut command);
    common::pipe_to_file(
        &mut command,
        &output.join("stdout.log"),
        &output.join("stderr.log"),
    )?;
    guard()?;
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 RustFS（PID 未取得，现场未知）：{error}"))?;
    let pid = child.id();
    let mut expected = None;
    let result = (|| {
        common::write_new(
            &output.join("spawned.json"),
            &json!({"pid": pid, "identity_pending": true}),
        )?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let identity = loop {
            if child.try_wait()?.is_some() {
                return Err("RustFS 在登记创建身份前退出".into());
            }
            if let Some(identity) = common::process_identity(pid)?
                && identity.get("executable").and_then(Value::as_str)
                    == request.pointer("/executable/path").and_then(Value::as_str)
            {
                break identity;
            }
            if Instant::now() >= deadline {
                return Err("新 RustFS 实际可执行文件与注册产物不符".into());
            }
            thread::sleep(Duration::from_millis(20));
        };
        expected = Some(identity.clone());
        let process_path = output.join("process.json");
        common::write_new(
            &process_path,
            &json!({
                "format_version": 1,
                "role": "rustfs",
                "scope_id": request["scope_id"],
                "lifecycle": "running",
                "identity": identity,
                "api_url": request["api_url"],
                "console_url": request["console_url"],
                "data_dir": request["data_directory"]["path"],
                "intent": common::binding(&intent_path)?,
            }),
        )?;
        let launch_path = output.join("launch.json");
        common::write_new(
            &launch_path,
            &json!({
                "format_version": 1,
                "scope_id": request["scope_id"],
                "identity": identity,
                "arguments": args,
                "credential_files": request["credential_files"],
            }),
        )?;
        let timeout = Duration::from_secs(
            request
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .ok_or("存储就绪期限无效")?,
        );
        let deadline = Instant::now() + timeout;
        let urls = [
            request["api_url"].as_str().unwrap().to_owned(),
            request["console_url"].as_str().unwrap().to_owned(),
        ];
        loop {
            if child.try_wait()?.is_some() {
                return Err("RustFS 就绪前退出".into());
            }
            if common::process_identity(pid)?.as_ref() != Some(&identity) {
                return Err("RustFS 就绪前实际身份变化".into());
            }
            if urls
                .iter()
                .all(|url| common::port_ready(url, Duration::from_millis(200)))
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err("RustFS 未在明确时限内监听登记端口".into());
            }
            thread::sleep(Duration::from_millis(100));
        }
        guard()?;
        if !running(&identity, request, true)? {
            return Err("存储最终就绪观察已退出".into());
        }
        let storage = json!({
            "identity": identity,
            "sha256": request["executable"]["sha256"],
            "process_receipt": common::binding(&process_path)?,
            "launch_receipt": common::binding(&launch_path)?,
        });
        common::write_new(
            &output.join("ready.json"),
            &json!({
                "storage": storage,
                "request": registration,
                "controller": controller,
                "attempt": number,
                "listeners": [request["api_url"], request["console_url"]],
            }),
        )?;
        Ok(storage)
    })();
    if let Err(original) = result {
        let cleanup = cleanup_child(&mut child, pid, expected.as_ref(), &intent_path, output);
        return match cleanup {
            Ok(()) => Err(original),
            Err(error) => Err(format!("{original}；RustFS 精确回收失败：{error}").into()),
        };
    }
    result
}

fn cleanup_child(
    child: &mut Child,
    pid: u32,
    identity: Option<&Value>,
    intent: &Path,
    output: &Path,
) -> Result<()> {
    let termination = if let Some(identity) = identity {
        common::terminate_tree(identity).map(|_| ())
    } else {
        if child.try_wait()?.is_none() {
            common::terminate_pid_tree(pid)?;
        }
        Ok(())
    };
    let status = child.wait().ok();
    common::write_new(
        &output.join("failure-cleanup.json"),
        &json!({
            "intent": common::binding(intent)?,
            "identity": identity,
            "pid": pid,
            "returncode": status.and_then(|status| status.code()).unwrap_or(-1),
        }),
    )?;
    termination
}
