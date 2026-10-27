//! monitor 拥有完整进程树；supervisor 仅在候选授权后启动产品。
use super::{
    launch,
    operation::{self, Operation},
    process,
    role::Role,
    tree,
};
use crate::{
    Result,
    process::{ChildGroup, stop_child},
    recovery::clone::native::{json, resources::common},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

fn inputs(args: &Value) -> Result<(Role, PathBuf, PathBuf, String)> {
    json::exact(
        args,
        &["backend", "directory", "operation_id", "role"],
        "角色监督请求",
    )?;
    if !super::documents::hex(&args["operation_id"], 32) {
        return Err("角色监督操作ID无效".into());
    }
    let backend = operation::canonical(Path::new(args["backend"].as_str().ok_or("缺少后端目录")?))?;
    let directory =
        operation::canonical(Path::new(args["directory"].as_str().ok_or("缺少运行目录")?))?;
    let role = Role::parse(&args["role"])?;
    // 前端新命令沿用发布者冻结的描述符拼法，核验真实路径后不重签输入。
    let (backend, directory) = if role == Role::Frontend {
        (
            PathBuf::from(args["backend"].as_str().ok_or("缺少后端目录")?),
            PathBuf::from(args["directory"].as_str().ok_or("缺少运行目录")?),
        )
    } else {
        (backend, directory)
    };
    Ok((
        role,
        backend,
        directory,
        args["operation_id"].as_str().ok_or("缺少操作ID")?.into(),
    ))
}
fn operation(role: Role, backend: &Path, directory: &Path, id: &str) -> Result<Operation> {
    super::runtime::load(role, backend, directory)?;
    let op = Operation::read(role, directory, &operation::source(directory)?)?;
    op.validate("request", Value::Null)?;
    if op.id()? != id
        || op.request["operation"]
            != if role == Role::Frontend {
                "run"
            } else {
                "start"
            }
        || op.request["backend_root"] != json!(backend)
    {
        return Err("角色监督进程不属于当前start操作".into());
    }
    Ok(op)
}
pub(super) fn monitor(args: &Value) -> Result<Value> {
    let (role, backend, directory, id) = inputs(args)?;
    let op = operation(role, &backend, &directory, &id)?;
    let monitor = process::current()?;
    let mut command = launch::command("supervise", args, &directory)?;
    if role == Role::Frontend {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }
    let environment = super::environment::Environment::operation(&op.request)?;
    if let Some(environment) = &environment {
        environment.apply(&mut command)?;
    }
    let mut child = ChildGroup::new()?.spawn(command)?;
    let supervisor = common::process_identity(child.id())?.ok_or("角色监督进程登记前退出")?;
    let ready = json!({"format_version":1,"operation_id":id,"directory":directory,"role":role.name(),"scope_id":op.request["scope_id"],"supervisor":supervisor,"monitor":monitor});
    let mut members = vec![supervisor.clone()];
    let result: Result<()> = (|| {
        operation::write_new(&tree::path(&directory, role, &id, "ready"), &ready)?;
        loop {
            for pid in child.member_pids()? {
                if let Some(identity) = common::process_identity(pid)?
                    && !members.contains(&identity)
                {
                    members.push(identity);
                }
            }
            if child.try_wait()?.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    })();
    // 即使直接子进程自然退出，也必须关闭仍留在 Job/group 内的后代。
    let cleanup = stop_child(&mut child);
    drop(child);
    let mut error = result.err().or_else(|| cleanup.err());
    let until = Instant::now() + Duration::from_secs(5);
    for identity in &members {
        while process::alive(identity)? {
            if Instant::now() >= until {
                error = Some("角色完整成员尚未退出".into());
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    let proof = json!({"format_version":1,"operation_id":id,"directory":directory,"role":role.name(),"scope_id":op.request["scope_id"],"supervisor":supervisor,"monitor":monitor,"status":if error.is_none(){"stopped"}else{"failed"},"members":members,"error_type":error.as_ref().map(|_|"MemberCleanupFailed")});
    operation::write_new(&tree::path(&directory, role, &id, "stopped"), &proof)?;
    if let Some(error) = error {
        return Err(error);
    }
    Ok(Value::Null)
}
pub(super) fn run(args: &Value) -> Result<Value> {
    let (role, backend, directory, id) = inputs(args)?;
    let op = operation(role, &backend, &directory, &id)?;
    let identity = process::current()?;
    let ready_path = tree::path(&directory, role, &id, "ready");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready_path.exists() {
        if Instant::now() >= deadline {
            return Err("角色成员监督器登记超时".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let ready = operation::read(&ready_path)?;
    if ready["supervisor"] != identity
        || ready["operation_id"] != id
        || ready["directory"] != json!(directory)
        || !process::alive(&ready["monitor"])?
    {
        return Err("角色成员监督器归属无效".into());
    }
    let mut candidate = op.bound(&op.role.kind("candidate"))?;
    candidate["identity"] = identity.clone();
    candidate["created_at_ns"] = json!(operation::now());
    op.publish("candidate.json", &candidate, false)?;
    loop {
        if op.path.join("supervisor.json").exists() {
            let authorized = op.child("supervisor.json")?;
            if authorized["identity"] != identity
                || authorized["candidate_sha256"] != super::documents::hash(&candidate)?
            {
                return Err("角色监督授权与实际创建身份不匹配".into());
            }
            break;
        }
        if Instant::now() >= deadline || !op.path.exists() {
            return Err("角色未获得授权，未启动产品".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let outcome = run_product(&backend, &op, &identity, &ready["monitor"]);
    if let Err(error) = outcome {
        // 原始错误可能来自文件配置；公开结果只保存固定分类与处理建议。
        let message = if error.to_string().contains("就绪超时") {
            format!(
                "{}就绪超时；已退出启动监督器，等待完整成员关闭证明",
                role.label()
            )
        } else {
            format!(
                "{}启动或监督失败；保留日志、完整成员证明和控制锁，显式核对",
                role.label()
            )
        };
        if op.path.exists() && !op.path.join("result.json").exists() {
            let result = op.result("unknown", Value::Null, Some(&message), false)?;
            op.publish("result.json", &result, false)?;
        }
        return Err(super::Rejection(message).into());
    }
    Ok(Value::Null)
}
fn progress(backend: &Path, op: &Operation, phase: &str, identity: &Value) -> Result<()> {
    let guard = super::controller::guard(op.role, op.directory(), Duration::from_secs(10))?;
    let current = operation(op.role, backend, op.directory(), op.id()?)?;
    let mut value = current.bound(&op.role.kind("progress"))?;
    value["phase"] = json!(phase);
    value["identity"] = identity.clone();
    value["supervisor_sha256"] = json!(super::documents::hash(&current.child("supervisor.json")?)?);
    value["updated_at_ns"] = json!(operation::now());
    current.publish("progress.json", &value, true)?;
    guard.check()
}
fn run_product(backend: &Path, op: &Operation, supervisor: &Value, monitor: &Value) -> Result<()> {
    let runtime = super::runtime::load(op.role, backend, op.directory())?;
    if op.role == Role::Frontend {
        return run_frontend(backend, op, supervisor, monitor, &runtime);
    }
    progress(backend, op, "launching", &Value::Null)?;
    let executable = op.request[op.role.artifact()]["path"]
        .as_str()
        .ok_or("缺少角色产物")?;
    let log = fs::OpenOptions::new().write(true).create_new(true).open(
        op.directory()
            .join(op.request["log"].as_str().ok_or("缺少日志")?),
    )?;
    let mut command = std::process::Command::new(executable);
    launch::environment(&mut command);
    let environment = super::environment::Environment::operation(&op.request)?;
    if let Some(environment) = &environment {
        environment.apply(&mut command)?;
    }
    command
        .arg("--config-dir")
        .arg(runtime[op.role.config()?].as_str().ok_or("缺少产品配置")?)
        .current_dir(backend)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn()?;
    let product = common::process_identity(child.id())?.ok_or("角色在登记前退出")?;
    if product["executable"] != op.request[op.role.artifact()]["path"] {
        return Err("角色产物与实际创建身份不同".into());
    }
    let tree = tree::register(op, supervisor, monitor, &product)?;
    progress(backend, op, "started", &product)?;
    let url = op.request[op.role.ready()].as_str().ok_or("缺少就绪端点")?;
    let deadline = Instant::now()
        + Duration::from_secs_f64(op.request["timeout_seconds"].as_f64().ok_or("无效超时")?);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Err("角色就绪前退出".into());
        }
        require_monitor(monitor)?;
        if super::controller::ready(url)? {
            process::listener(&product, url)?;
            progress(backend, op, "ready", &product)?;
            let guard = super::controller::guard(op.role, op.directory(), Duration::from_secs(10))?;
            super::runtime::load(op.role, backend, op.directory())?;
            op.publish(
                "result.json",
                &op.result("running", product.clone(), None, false)?,
                false,
            )?;
            guard.check()?;
            drop(guard);
            return supervise_product(&mut child, &tree).map(|_| ());
        }
        thread::sleep(Duration::from_millis(100));
    }
    // supervisor 退出后 monitor 的 Job/group 回收全部成员；这里不按孤立 PID 降级清理。
    Err("角色就绪超时".into())
}
fn run_frontend(
    backend: &Path,
    op: &Operation,
    supervisor: &Value,
    monitor: &Value,
    runtime: &Value,
) -> Result<()> {
    progress(backend, op, "launching", &Value::Null)?;
    let argv = runtime["command"]
        .as_array()
        .ok_or("前端命令参数必须为数组")?;
    let executable = argv.first().and_then(Value::as_str).ok_or("前端命令为空")?;
    let mut command = std::process::Command::new(executable);
    command.env_clear();
    for arg in &argv[1..] {
        command.arg(arg.as_str().ok_or("前端命令参数不是字符串")?);
    }
    for (key, value) in runtime["environment"].as_object().ok_or("前端环境无效")? {
        command.env(key, value.as_str().ok_or("前端环境值不是字符串")?);
    }
    command
        .current_dir(runtime["cwd"].as_str().ok_or("前端工作目录缺失")?)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn()?;
    let product = common::process_identity(child.id())?.ok_or("前端产品登记前退出")?;
    if product["executable"] != op.request[op.role.artifact()]["path"] {
        return Err("前端实际产物与冻结启动器不一致".into());
    }
    let tree = tree::register(op, supervisor, monitor, &product)?;
    progress(backend, op, "started", &product)?;
    let code = supervise_product(&mut child, &tree)?;
    super::runtime::load(op.role, backend, op.directory())?;
    progress(backend, op, "completed", &product)?;
    let guard = super::controller::guard(op.role, op.directory(), Duration::from_secs(10))?;
    op.publish(
        "result.json",
        &op.result(
            "stopped",
            Value::Null,
            (code != 0).then_some("前端命令退出失败；核对脱敏日志和原始退出状态，禁止重放"),
            false,
        )?,
        false,
    )?;
    guard.check()
}

fn supervise_product(child: &mut std::process::Child, tree: &Value) -> Result<i32> {
    let directory = Path::new(tree["runtime_directory"].as_str().ok_or("目录无效")?);
    let id = tree["operation_id"].as_str().ok_or("操作ID无效")?;
    let control = directory.join(format!(
        "{}-tree-{id}-control.json",
        Role::parse(&tree["role"])?.name()
    ));
    let (code, termination) = loop {
        if let Some(status) = child.try_wait()? {
            break (status.code().unwrap_or(1), "natural");
        }
        require_monitor(&tree["monitor"])?;
        if control.exists() {
            let request = operation::read(&control)?;
            json::exact(
                &request,
                &[
                    "format_version",
                    "kind",
                    "operation_id",
                    "mode",
                    "requested_at_ns",
                ],
                "角色停止请求",
            )?;
            if request["format_version"].as_u64() != Some(1)
                || request["kind"] != "full-stack-process-tree-control"
                || request["operation_id"] != tree["operation_id"]
                || !matches!(request["mode"].as_str(), Some("normal" | "crash"))
                || !super::documents::positive(&request["requested_at_ns"])
            {
                return Err("角色停止请求无效".into());
            }
            break stop_product(child, request["mode"] == "crash")?;
        }
        thread::sleep(Duration::from_millis(20));
    };
    operation::write_new(
        &directory.join(format!(
            "{}-tree-{id}-result.json",
            Role::parse(&tree["role"])?.name()
        )),
        &json!({"format_version":1,"kind":"full-stack-process-tree-result","operation_id":id,"process":tree["process"],"exit_code":code,"termination":termination}),
    )?;
    Ok(code)
}

fn stop_product(child: &mut std::process::Child, crash: bool) -> Result<(i32, &'static str)> {
    #[cfg(unix)]
    if !crash {
        use nix::{
            sys::signal::{Signal, kill},
            unistd::Pid,
        };
        kill(Pid::from_raw(i32::try_from(child.id())?), Signal::SIGTERM)?;
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Some(status) = child.try_wait()? {
                return Ok((status.code().unwrap_or(1), "normal"));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(windows)]
    let _ = crash;
    child.kill()?;
    let status = child.wait()?;
    Ok((status.code().unwrap_or(1), "forced"))
}

fn require_monitor(monitor: &Value) -> Result<()> {
    if process::alive(monitor)? {
        return Ok(());
    }
    #[cfg(unix)]
    {
        // 树外控制器失联时不能留下后代；supervisor 自身是受管进程组的组长。
        use nix::{
            sys::signal::{Signal, kill},
            unistd::Pid,
        };
        kill(
            Pid::from_raw(-i32::try_from(std::process::id())?),
            Signal::SIGKILL,
        )?;
    }
    Err("角色成员监督器退出，完整回收结果未知；保留现场".into())
}
