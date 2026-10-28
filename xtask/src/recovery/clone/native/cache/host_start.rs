//! 正式 cache 启动：先登记完整 launcher，再发送授权，accepted 后才发布 runtime。
use super::{
    super::{digest, json as doc, resources::common},
    evidence, host_control, linux_write, model, native_tool,
};
use crate::Result;
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::Write,
    path::Path,
    process::{Child, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn start(
    backend: &Path,
    request: &Value,
    private: &Value,
    output: &Path,
    mut guard: impl FnMut() -> Result<()>,
) -> Result<Value> {
    model::validate(backend, request, true)?;
    if request["format_version"] != 2 {
        return Err("新缓存启动只接受 v2 原生请求".into());
    }
    let tool = native_tool::from_request(request)?;
    tool.finish(backend)?;
    super::request::run(
        "environment-select",
        &json!({"private":{"environment":private}}),
    )?;
    common::local_directory(backend, output, "缓存启动输出")?;
    // 同一绑定 ELF 的只读来源描述在发布 launch 前核验。
    let _observer = super::linux_observer::Observer::new(backend, request)?;
    let launch = linux_write::Launch {
        format_version: 2,
        request: request.clone(),
        request_sha256: digest::plan_hash(request)?,
        windows_output: output.to_str().ok_or("缓存路径不是UTF-8")?.into(),
        linux_output: model::linux_path(output.to_str().ok_or("缓存路径不是UTF-8")?)?.into(),
        native_tool: tool.clone(),
    };
    linux_write::publish(
        &output.join("launch-request.json"),
        &serde_json::to_value(launch)?,
    )?;
    guard()?;
    host_control::bind_and_close(request)?;
    let args = evidence::command(request, output, "serve")?;
    linux_write::publish(
        &output.join("intent.json"),
        &json!({"request_sha256":digest::plan_hash(request)?,
        "arguments":args,"launch":common::binding(&output.join("launch-request.json"))?}),
    )?;
    let mut attempt = Attempt::default();
    let mut result = launch_and_accept(
        backend,
        request,
        private,
        output,
        &args,
        &mut attempt,
        &mut guard,
    );
    if result.is_err() {
        result = failed(backend, request, output, &mut attempt, result);
    }
    tool.finish(backend)?;
    model::validate(backend, request, true)?;
    result
}

fn failed(
    backend: &Path,
    request: &Value,
    output: &Path,
    attempt: &mut Attempt,
    mut result: Result<Value>,
) -> Result<Value> {
    if let Some(child) = attempt.child.as_mut() {
        drop(child.stdin.take());
    }
    let cleanup = cleanup(
        backend,
        request,
        output,
        attempt.child.as_mut(),
        attempt.runtime.as_ref(),
        attempt.authorized,
    );
    linux_write::publish(
        &output.join("failure.json"),
        &json!({"error_type":"NativeCacheFailure",
        "launcher":attempt.trusted,"request_sha256":digest::plan_hash(request)?,"runtime_recorded":attempt.runtime.is_some(),
        "cleanup_completed":cleanup.is_ok()}),
    )?;
    if output.join("linux-failure.json").try_exists()? {
        let failure = doc::read(&output.join("linux-failure.json"))?;
        if failure["request_sha256"] != digest::plan_hash(request)? {
            return Err("缓存失败收据不属于原请求".into());
        }
        let code = result
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<super::process::ControlFailure>())
            .and_then(|error| error.0);
        result = Err(Box::new(super::process::RemoteFailure {
            code,
            diagnostic: super::diagnostic::Diagnostic::decode(&failure["diagnostic"])?,
        }));
    }
    result
}

#[derive(Default)]
struct Attempt {
    child: Option<Child>,
    trusted: Value,
    runtime: Option<Value>,
    authorized: bool,
}

fn launch_and_accept(
    backend: &Path,
    request: &Value,
    private: &Value,
    output: &Path,
    args: &[String],
    attempt: &mut Attempt,
    guard: &mut impl FnMut() -> Result<()>,
) -> Result<Value> {
    authorize(request, private, output, args, attempt)?;
    let child = attempt.child.as_mut().ok_or("缓存 launcher 缺失")?;
    let runtime = &mut attempt.runtime;
    let deadline = Instant::now()
        + Duration::from_secs(request["timeout_seconds"].as_u64().ok_or("缓存超时无效")?);
    wait_file(
        child,
        &output.join("linux-process.json"),
        deadline,
        super::diagnostic::Category::ProcessReceiptTimeout,
    )?;
    let initial = evidence::inspect(request, output)?;
    *runtime = Some(initial.clone());
    let observed = loop {
        match host_control::observe(backend, request, &initial) {
            Ok(value) => break value,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused) =>
            {
                if child.try_wait()?.is_some() || Instant::now() >= deadline {
                    return Err("缓存认证就绪超时".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    };
    guard()?;
    linux_write::publish(
        &output.join("acknowledged.json"),
        &json!({"request_sha256":digest::plan_hash(request)?,"identity":observed["linux_identity"]}),
    )?;
    wait_file(
        child,
        &output.join("accepted.json"),
        deadline,
        super::diagnostic::Category::AcceptanceTimeout,
    )?;
    if doc::read(&output.join("accepted.json"))? != doc::read(&output.join("acknowledged.json"))? {
        return Err("缓存 accepted 与 acknowledged 不同".into());
    }
    native_tool::from_request(request)?.finish(backend)?;
    *runtime = Some(observed.clone());
    linux_write::publish(&output.join("runtime.json"), &observed)?;
    Ok(observed)
}

fn authorize(
    request: &Value,
    private: &Value,
    output: &Path,
    args: &[String],
    attempt: &mut Attempt,
) -> Result<()> {
    let Attempt {
        child,
        trusted,
        runtime: _,
        authorized,
    } = attempt;
    let stdout = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output.join("wsl.stdout.log"))?;
    let stderr = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output.join("wsl.stderr.log"))?;
    let mut command = crate::process::child_command(&args[0]);
    command
        .args(&args[1..])
        .current_dir(output)
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(stderr);
    restricted(&mut command, private)?;
    *child = Some(command.spawn()?);
    let child = child.as_mut().ok_or("缓存 launcher 缺失")?;
    let actual = common::process_identity(child.id())?.ok_or("缓存 WSL 创建身份缺失")?;
    if !doc::text(&actual["executable"], "WSL 实际产物")?
        .eq_ignore_ascii_case(doc::text(&request["wsl"]["path"], "WSL 绑定产物")?)
    {
        return Err("缓存 WSL 实际产物不同".into());
    }
    *trusted = actual.clone();
    linux_write::publish(
        &output.join("launcher.json"),
        &json!({"identity":actual,"intent":common::binding(&output.join("intent.json"))?}),
    )?;
    if crate::recovery::fixture_control::services::lifecycle::readonly::process::arguments(&actual)?
        != args
    {
        return Err("缓存 launcher 完整参数不同".into());
    }
    *authorized = true;
    writeln!(
        child.stdin.as_mut().ok_or("缓存授权管道缺失")?,
        "{}",
        json!({"request_sha256":digest::plan_hash(request)?,"output":output})
    )?;
    drop(child.stdin.take());

    Ok(())
}

fn restricted(command: &mut std::process::Command, private: &Value) -> Result<()> {
    let names = [
        "PATH",
        "PATHEXT",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "USERPROFILE",
        "HOME",
        "HOMEDRIVE",
        "HOMEPATH",
        "TEMP",
        "TMP",
    ];
    let host = std::env::vars_os()
        .filter(|(key, _)| names.contains(&key.to_string_lossy().to_ascii_uppercase().as_str()))
        .collect::<Vec<_>>();
    command.env_clear().envs(host);
    for (key, value) in doc::object(private, "缓存私有配置")? {
        command.env(key, doc::text(value, "缓存私有配置文本")?);
    }
    Ok(())
}

fn wait_file(
    child: &mut Child,
    path: &Path,
    deadline: Instant,
    category: super::diagnostic::Category,
) -> Result<()> {
    while !path.try_exists()? {
        if let Some(status) = child.try_wait()? {
            return Err(super::process::ControlFailure(status.code()).into());
        }
        if Instant::now() >= deadline {
            return Err(super::diagnostic::failure(
                category,
                super::diagnostic::Outcome::Unknown,
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn cleanup(
    backend: &Path,
    request: &Value,
    output: &Path,
    child: Option<&mut Child>,
    runtime: Option<&Value>,
    authorized: bool,
) -> Result<()> {
    let observed = if let Some(runtime) = runtime {
        Some(runtime.clone())
    } else if output.join("linux-process.json").try_exists()? {
        Some(evidence::inspect(request, output)?)
    } else {
        None
    };
    if let Some(runtime) = observed {
        let cleanup = output.join("failure-stop");
        std::fs::create_dir(&cleanup)?;
        host_control::stop(backend, request, &runtime, &cleanup)?;
        return Ok(());
    }
    let child = child.ok_or("缓存尚未取得自有 launcher，启动结果需核对")?;
    let deadline = Instant::now()
        + Duration::from_secs(request["timeout_seconds"].as_u64().ok_or("缓存超时无效")? + 20);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            return Err("缓存监督器清理未确认；禁止重放".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let failure = output.join("linux-failure.json");
    let not_started = !authorized && !output.join("linux-intent.json").try_exists()?;
    if !not_started && (!failure.try_exists()? || doc::read(&failure)?["cleanup_completed"] != true)
    {
        return Err("缓存缺少精确部分失败收据".into());
    }
    let code = status.code().ok_or("缓存 launcher 没有明确退出码")?;
    linux_write::publish(
        &output.join("partial.json"),
        &json!({
        "state":if not_started {"not_started"} else {"stopped"},
        "request_sha256":digest::plan_hash(request)?,"intent":common::binding(&output.join("intent.json"))?,
        "launcher_pid":child.id(),"launcher_returncode":code,"authorization_sent":authorized,
        "linux_cleanup":if not_started {Value::Null} else {common::binding(&failure)?}}),
    )?;
    evidence::inspect(request, output)?;
    Ok(())
}
