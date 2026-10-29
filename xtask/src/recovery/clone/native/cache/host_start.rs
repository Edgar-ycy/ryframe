//! 正式 cache 启动：先登记完整 launcher，再发送授权，accepted 后才发布 runtime。
use super::{
    super::{digest, json as doc, resources::common},
    diagnostic::{self, Category},
    evidence, host_control, linux_write, model, native_tool,
    start_deadline::Deadline,
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
    let args = prelaunch(request, output, &mut guard)?;
    let mut attempt = Attempt::default();
    let mut result = launch_and_accept(
        backend,
        request,
        private,
        output,
        &args,
        &mut attempt,
        &mut guard,
    )
    .and_then(|(observed, deadline)| {
        deadline.check(
            Instant::now(),
            super::diagnostic::Category::AcceptanceTimeout,
        )?;
        linux_write::publish(&output.join("runtime.json"), &observed)?;
        tool.finish(backend)?;
        model::validate(backend, request, true)?;
        deadline.check(
            Instant::now(),
            super::diagnostic::Category::AcceptanceTimeout,
        )?;
        Ok(observed)
    });
    if result.is_err() {
        result = failed(backend, request, output, &mut attempt, result);
        tool.finish(backend)?;
        model::validate(backend, request, true)?;
    }
    result
}

fn prelaunch(
    request: &Value,
    output: &Path,
    guard: &mut impl FnMut() -> Result<()>,
) -> Result<Vec<String>> {
    guard().map_err(|error| diagnostic::prelaunch(Category::PrelaunchGuardRejected, error))?;
    host_control::bind_and_close(request, host_control::PortCheck::BeforeStart)?;
    let args = evidence::command(request, output, "serve")
        .map_err(|error| diagnostic::prelaunch(Category::PrelaunchCommandRejected, error))?;
    (|| {
        linux_write::publish(
            &output.join("intent.json"),
            &json!({"request_sha256":digest::plan_hash(request)?,
            "arguments":args,"launch":common::binding(&output.join("launch-request.json"))?}),
        )
    })()
    .map_err(|error| diagnostic::prelaunch(Category::PrelaunchIntentFailed, error))?;
    Ok(args)
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
    let cleanup_detail = cleanup
        .as_ref()
        .err()
        .map(|error| super::cleanup_diagnostic::capture(error.as_ref()));
    linux_write::publish(
        &output.join("failure.json"),
        &json!({"error_type":"NativeCacheFailure",
        "launcher":attempt.trusted,"request_sha256":digest::plan_hash(request)?,"runtime_recorded":attempt.runtime.is_some(),
        "cleanup_completed":cleanup.is_ok()}),
    )?;
    if let Some(detail) = &cleanup_detail {
        linux_write::publish(
            &output.join("cleanup-diagnostic.json"),
            &json!({
            "request_sha256":digest::plan_hash(request)?,"launcher":attempt.trusted,"cleanup":detail}),
        )?;
    }
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
        let diagnostic = super::diagnostic::Diagnostic::decode(&failure["diagnostic"])?;
        let phase_timeout = result.as_ref().err().is_some_and(|error| {
            error
                .downcast_ref::<super::diagnostic::Diagnostic>()
                .is_some_and(|detail| {
                    matches!(
                        detail.category,
                        super::diagnostic::Category::ReadinessTimeout
                            | super::diagnostic::Category::AcceptanceTimeout
                    )
                })
        });
        if !phase_timeout {
            result = Err(Box::new(super::process::RemoteFailure { code, diagnostic }));
        }
    }
    if let Some(detail) = cleanup_detail {
        result = match result {
            Ok(value) => Ok(value),
            Err(error) => Err(super::cleanup_diagnostic::attach(error, detail)?),
        };
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
) -> Result<(Value, Deadline)> {
    authorize(request, private, output, args, attempt)?;
    let child = attempt.child.as_mut().ok_or("缓存 launcher 缺失")?;
    let runtime = &mut attempt.runtime;
    let deadline = Deadline::new(
        Instant::now(),
        Duration::from_secs(request["timeout_seconds"].as_u64().ok_or("缓存超时无效")?),
    );
    wait_file(
        child,
        &output.join("linux-process.json"),
        deadline,
        super::diagnostic::Category::ProcessReceiptTimeout,
    )?;
    let initial = evidence::inspect(request, output)?;
    *runtime = Some(initial.clone());
    let observed = ready(backend, request, &initial, child, deadline, guard)?;
    linux_write::publish(
        &output.join("acknowledged.json"),
        &json!({"request_sha256":digest::plan_hash(request)?,"identity":observed["linux_identity"]}),
    )?;
    let deadline = deadline.accepting(
        Instant::now(),
        super::diagnostic::Category::ReadinessTimeout,
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
    deadline.check(
        Instant::now(),
        super::diagnostic::Category::AcceptanceTimeout,
    )?;
    *runtime = Some(observed.clone());
    Ok((observed, deadline))
}

fn ready(
    backend: &Path,
    request: &Value,
    initial: &Value,
    child: &mut Child,
    deadline: Deadline,
    guard: &mut impl FnMut() -> Result<()>,
) -> Result<Value> {
    let observed = loop {
        deadline.check(
            Instant::now(),
            super::diagnostic::Category::ReadinessTimeout,
        )?;
        match host_control::observe(backend, request, initial) {
            Ok(value) => break value,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused) =>
            {
                if child.try_wait()?.is_some() {
                    return Err("缓存认证就绪超时".into());
                }
                deadline.check(
                    Instant::now(),
                    super::diagnostic::Category::ReadinessTimeout,
                )?;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    };
    deadline.check(
        Instant::now(),
        super::diagnostic::Category::ReadinessTimeout,
    )?;
    guard()?;
    deadline.check(
        Instant::now(),
        super::diagnostic::Category::ReadinessTimeout,
    )?;
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
    deadline: Deadline,
    category: super::diagnostic::Category,
) -> Result<()> {
    loop {
        let exists = path.try_exists()?;
        if exists {
            deadline.check(Instant::now(), category)?;
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(super::process::ControlFailure(status.code()).into());
        }
        deadline.check(Instant::now(), category)?;
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn cleanup(
    backend: &Path,
    request: &Value,
    output: &Path,
    child: Option<&mut Child>,
    runtime: Option<&Value>,
    authorized: bool,
) -> Result<()> {
    use super::cleanup_diagnostic::{Operation, context};
    let observed = if let Some(runtime) = runtime {
        Some(runtime.clone())
    } else if output.join("linux-process.json").try_exists()? {
        Some(evidence::inspect(request, output).map_err(|e| context(Operation::Inspect, e))?)
    } else {
        None
    };
    if let Some(runtime) = observed {
        let cleanup = output.join("failure-stop");
        std::fs::create_dir(&cleanup).map_err(|e| context(Operation::CreateDirectory, e.into()))?;
        host_control::stop(backend, request, &runtime, &cleanup)
            .map_err(|e| context(Operation::Stop, e))?;
        return Ok(());
    }
    let child = child.ok_or_else(|| {
        context(
            Operation::WaitLauncher,
            "缓存尚未取得自有 launcher，启动结果需核对".into(),
        )
    })?;
    let deadline = Instant::now()
        + Duration::from_secs(request["timeout_seconds"].as_u64().ok_or("缓存超时无效")? + 20);
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| context(Operation::WaitLauncher, e.into()))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            return Err(context(
                Operation::WaitLauncher,
                "缓存监督器清理未确认；禁止重放".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let failure = output.join("linux-failure.json");
    let not_started = !authorized && !output.join("linux-intent.json").try_exists()?;
    if !not_started
        && (!failure.try_exists()?
            || doc::read(&failure).map_err(|e| context(Operation::ReadFailure, e))?["cleanup_completed"]
                != true)
    {
        return Err(context(
            Operation::ReadFailure,
            "缓存缺少精确部分失败收据".into(),
        ));
    }
    let code = status.code().ok_or("缓存 launcher 没有明确退出码")?;
    linux_write::publish(
        &output.join("partial.json"),
        &json!({
        "state":if not_started {"not_started"} else {"stopped"},
        "request_sha256":digest::plan_hash(request)?,"intent":common::binding(&output.join("intent.json"))?,
        "launcher_pid":child.id(),"launcher_returncode":code,"authorization_sent":authorized,
        "linux_cleanup":if not_started {Value::Null} else {common::binding(&failure)?}}),
    ).map_err(|e| context(Operation::PublishPartial, e))?;
    evidence::inspect(request, output).map_err(|e| context(Operation::Inspect, e))?;
    Ok(())
}

#[cfg(test)]
#[test]
fn prelaunch_guard_failure_keeps_stage_and_safe_cause_without_publishing_intent() {
    use super::diagnostic::{primitive_detail, primitive_error, primitive_message};
    let base = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&base).unwrap();
    let directory = tempfile::tempdir_in(base).unwrap();
    for name in ["request.json", "launch-request.json"] {
        std::fs::write(directory.path().join(name), b"original-prelaunch-input").unwrap();
    }
    for (message, expected) in [
        ("生命周期私有环境在执行期间变化", "environment_changed"),
        (
            "生命周期私有环境在执行期间变化; password=private-marker",
            "rejected",
        ),
    ] {
        let error =
            prelaunch(&json!({}), directory.path(), &mut || Err(message.into())).unwrap_err();
        let (detail, code) = primitive_detail(error.as_ref());
        assert_eq!(detail["category"], "prelaunch_guard_rejected");
        assert_eq!(detail["cause"], expected);
        assert_eq!(detail["write_outcome"], "not_started");
        assert!(code.is_null());
        let decoded = primitive_error(&detail, &code).unwrap();
        assert!(!format!("{decoded:?} {decoded}").contains("private-marker"));
        assert!(
            primitive_message(decoded.as_ref())
                .unwrap()
                .contains("prelaunch-guard")
        );
        for key in ["cause", "cause_sha256", "advice"] {
            let mut changed = detail.clone();
            changed[key] = json!("private-marker");
            assert!(
                !primitive_error(&changed, &code)
                    .unwrap_err()
                    .to_string()
                    .contains("private-marker")
            );
        }
        let mut missing = detail.clone();
        missing.as_object_mut().unwrap().remove("cause");
        assert!(primitive_error(&missing, &code).is_err());
        assert!(primitive_error(&detail, &json!(1)).is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
        for name in ["request.json", "launch-request.json"] {
            assert_eq!(
                std::fs::read(directory.path().join(name)).unwrap(),
                b"original-prelaunch-input"
            );
        }
    }
}

#[cfg(test)]
#[test]
fn an_existing_receipt_does_not_turn_an_expired_phase_into_success() {
    let base = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&base).unwrap();
    let directory = tempfile::tempdir_in(base).unwrap();
    let path = directory.path().join("receipt.json");
    std::fs::write(&path, b"{}").unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--list")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for category in [
        super::diagnostic::Category::ReadinessTimeout,
        super::diagnostic::Category::AcceptanceTimeout,
    ] {
        let result = wait_file(
            &mut child,
            &path,
            Deadline::new(Instant::now(), Duration::ZERO),
            category,
        );
        let error = result.unwrap_err();
        let diagnostic = error
            .downcast_ref::<super::diagnostic::Diagnostic>()
            .unwrap();
        assert_eq!(diagnostic.value()["category"], json!(category));
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    }
    child.wait().unwrap();
}

#[cfg(test)]
#[test]
fn cleanup_diagnostics_do_not_replace_the_initiating_windows_phase_timeout() {
    use super::diagnostic::{Category, Diagnostic, Outcome, failure};
    let backend = crate::workspace::root_dir();
    let base = backend.join(".local-tests");
    std::fs::create_dir_all(&base).unwrap();
    let temporary = tempfile::tempdir_in(base).unwrap();
    for (index, category) in [
        Category::ReadinessTimeout,
        Category::AcceptanceTimeout,
        Category::RequestRejected,
    ]
    .into_iter()
    .enumerate()
    {
        let output = temporary.path().join(index.to_string());
        std::fs::create_dir(&output).unwrap();
        let request = json!({});
        let remote = Diagnostic {
            category: Category::AcknowledgementTimeout,
            write_outcome: Outcome::Stopped,
            os_code: None,
        };
        let receipt = json!({"request_sha256":digest::plan_hash(&request).unwrap(), "diagnostic":remote.value()});
        doc::write_new(&output.join("linux-failure.json"), &receipt).unwrap();
        let result = failed(
            &backend,
            &request,
            &output,
            &mut Attempt::default(),
            Err(failure(category, Outcome::Unknown)),
        );
        let error = result.unwrap_err();
        let (detail, _) = super::diagnostic::primitive_detail(error.as_ref());
        assert_eq!(
            detail["category"],
            if index < 2 {
                json!(category)
            } else {
                remote.value()["category"].clone()
            }
        );
        let cleanup = super::cleanup_diagnostic::detail(error.as_ref());
        super::cleanup_diagnostic::validate(&cleanup).unwrap();
        assert_eq!(cleanup["operation"], "wait_launcher");
        assert_eq!(
            doc::read(&output.join("failure.json")).unwrap()["cleanup_completed"],
            false
        );
        assert_eq!(
            doc::read(&output.join("linux-failure.json")).unwrap(),
            receipt
        );
    }
}
