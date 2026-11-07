//! supervisor 只启动固定 RustFS；实际产品继承既有 Job 或本进程的 session。
use super::*;
use std::{
    fs::OpenOptions,
    process::{Child, Stdio},
};

pub(super) fn run(actor: &actor::Actor) -> Result<()> {
    #[cfg(unix)]
    nix::unistd::setsid()?;
    let supervisor = crate::recovery::clone::native::lifecycle::identity::current()?;
    let monitor = wait_monitor(actor, &supervisor)?;
    check(!actor.cancelled()?, "RustFS 启动已取消，未启动产品")?;
    let mut child = product(actor)?;
    #[cfg(target_os = "linux")]
    let fd = match rustix::process::pidfd_open(
        rustix::process::Pid::from_child(&child),
        rustix::process::PidfdFlags::empty(),
    ) {
        Ok(fd) => fd,
        Err(error) => {
            let killed = child.kill();
            let waited = child.wait();
            killed?;
            waited?;
            return Err(error.into());
        }
    };
    let outcome = (|| {
        let identity = product_identity(actor, &mut child)?;
        receipts(actor, &identity)?;
        acknowledge(actor, &identity, &monitor)?;
        let tree = json!({"format_version":2,"kind":"full-stack-process-tree","runtime_directory":actor.output,
            "role":"rustfs","scope_id":actor.request["scope_id"],"operation_id":actor.operation_id,
            "supervisor":supervisor,"process":identity,"monitor":monitor,"group_id":supervisor["pid"]});
        actor.verify_bound()?;
        document::write_new(&actor.path("rustfs-tree.json"), &tree)?;
        #[cfg(target_os = "linux")]
        let result = supervise(actor, &tree, &mut child, &fd)?;
        #[cfg(not(target_os = "linux"))]
        let result = supervise(actor, &tree, &mut child)?;
        document::write_new(
            &actor.path(&format!("rustfs-tree-{}-result.json", actor.operation_id)),
            &json!({"format_version":1,"kind":"full-stack-process-tree-result","operation_id":actor.operation_id,
            "process":identity,"exit_code":result.0,"termination":result.1}),
        )?;
        Ok(())
    })();
    if outcome.is_err() {
        // 只清理本次自有句柄；其他成员仍由树外 monitor 统一确认。
        #[cfg(target_os = "linux")]
        platform::signal(&fd, rustix::process::Signal::KILL)?;
        #[cfg(not(target_os = "linux"))]
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        child.wait()?;
    }
    outcome
}

fn wait_monitor(actor: &actor::Actor, supervisor: &Value) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(actor.timeout()?);
    while !actor.member_path("ready").try_exists()? {
        check(
            Instant::now() < deadline && !actor.cancelled()?,
            "RustFS monitor 登记超时或取消",
        )?;
        std::thread::sleep(Duration::from_millis(20));
    }
    let value = document::read(&actor.member_path("ready"))?;
    document::exact(
        &value,
        &[
            "format_version",
            "operation_id",
            "directory",
            "role",
            "scope_id",
            "supervisor",
            "monitor",
        ],
        "RustFS monitor 登记",
    )?;
    check(
        value["format_version"] == 1
            && value["supervisor"] == *supervisor
            && value["operation_id"] == actor.operation_id
            && value["directory"] == json!(actor.output)
            && value["role"] == "rustfs"
            && value["scope_id"] == actor.request["scope_id"],
        "RustFS monitor 归属无效",
    )?;
    actor::require(&value["monitor"])?;
    Ok(value["monitor"].clone())
}

fn product(actor: &actor::Actor) -> Result<Child> {
    let args = actor.arguments()?;
    let mut command = crate::process::child_command(&args[0]);
    command
        .args(&args[1..])
        .current_dir(&actor.output)
        .stdin(Stdio::null());
    launch::environment(&mut command, actor)?;
    let stdout = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(actor.path("stdout.log"))?;
    // 保留原监督路径的合并 stdout 以及独占 stderr 文件布局。
    let _stderr = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(actor.path("stderr.log"))?;
    command.stderr(stdout.try_clone()?).stdout(stdout);
    actor.verify_bound()?;
    command.spawn().map_err(Into::into)
}

fn product_identity(actor: &actor::Actor, child: &mut Child) -> Result<Value> {
    document::write_new(
        &actor.path("spawned.json"),
        &json!({"pid":child.id(),"identity_pending":true}),
    )?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(identity) = common::process_identity(child.id())? {
            check(
                Path::new(document::text(&identity["executable"], "RustFS 实际映像")?)
                    == Path::new(document::text(
                        &actor.request["executable"]["path"],
                        "冻结映像",
                    )?),
                "RustFS 实际映像不同",
            )?;
            return Ok(identity);
        }
        check(Instant::now() < deadline, "RustFS 创建身份未登记")?;
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn receipts(actor: &actor::Actor, identity: &Value) -> Result<()> {
    document::write_new(
        &actor.path("process.json"),
        &json!({"format_version":1,"role":"rustfs",
        "scope_id":actor.request["scope_id"],"lifecycle":"running","identity":identity,
        "api_url":actor.request["api_url"],"console_url":actor.request["console_url"],
        "data_dir":actor.request["data_directory"]["path"],"intent":common::binding(&actor.path("intent.json"))?}),
    )?;
    document::write_new(
        &actor.path("launch.json"),
        &json!({"format_version":1,"scope_id":actor.request["scope_id"],
        "identity":identity,"arguments":actor.arguments()?,"credential_files":actor.request["credential_files"]}),
    )?;
    document::write_new(
        &actor.path("rustfs.json"),
        &json!({"format_version":1,"role":"rustfs",
        "scope_id":actor.request["scope_id"],"identity":identity}),
    )
}

fn acknowledge(actor: &actor::Actor, identity: &Value, monitor: &Value) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(actor.timeout()?);
    while !actor.member_path("product").try_exists()? {
        actor::require(monitor)?;
        check(
            Instant::now() < deadline && !actor.cancelled()?,
            "RustFS 产品成员句柄登记超时",
        )?;
        std::thread::sleep(Duration::from_millis(20));
    }
    let value = document::read(&actor.member_path("product"))?;
    document::exact(
        &value,
        &["operation_id", "identity", "monitor"],
        "RustFS 产品句柄登记",
    )?;
    check(
        value == json!({"operation_id":actor.operation_id,"identity":identity,"monitor":monitor}),
        "RustFS 产品句柄登记不同于本次创建",
    )
}

fn control(actor: &actor::Actor) -> Result<Option<bool>> {
    let path = actor.path(&format!("rustfs-tree-{}-control.json", actor.operation_id));
    if !path.try_exists()? {
        return Ok(None);
    }
    let value = document::read(&path)?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "operation_id",
            "mode",
            "requested_at_ns",
        ],
        "RustFS 停止控制",
    )?;
    check(
        value["format_version"] == 1
            && value["kind"] == "full-stack-process-tree-control"
            && value["operation_id"] == actor.operation_id
            && matches!(value["mode"].as_str(), Some("normal" | "crash"))
            && value["requested_at_ns"].as_u64().is_some_and(|v| v > 0),
        "RustFS 停止控制无效",
    )?;
    Ok(Some(value["mode"] == "crash"))
}

#[cfg(target_os = "linux")]
fn supervise(
    actor: &actor::Actor,
    tree: &Value,
    child: &mut Child,
    fd: &std::os::fd::OwnedFd,
) -> Result<(i32, &'static str)> {
    let mut termination = "natural";
    loop {
        if platform::exited(fd)? {
            break;
        }
        if actor::require(&tree["monitor"]).is_err() {
            // monitor 失联时，本进程仍占有其 session/group，不能留下未跟踪后代。
            nix::sys::signal::killpg(nix::unistd::getpid(), nix::sys::signal::Signal::SIGKILL)?;
            return Err("RustFS monitor 失联".into());
        }
        if let Some(crash) = control(actor)? {
            let normal = !crash;
            platform::signal(
                fd,
                if normal {
                    rustix::process::Signal::TERM
                } else {
                    rustix::process::Signal::KILL
                },
            )?;
            let deadline = Instant::now() + Duration::from_secs(1);
            while !platform::exited(fd)? && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            termination = if normal && platform::exited(fd)? {
                "normal"
            } else {
                "forced"
            };
            if !platform::exited(fd)? {
                platform::signal(fd, rustix::process::Signal::KILL)?;
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while !platform::exited(fd)? {
                check(Instant::now() < deadline, "RustFS 产品 pidfd 退出未确认")?;
                std::thread::sleep(Duration::from_millis(20));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    use std::os::unix::process::ExitStatusExt;
    let status = child.wait()?;
    Ok((
        status
            .code()
            .unwrap_or_else(|| -status.signal().unwrap_or(1)),
        termination,
    ))
}

#[cfg(not(target_os = "linux"))]
fn supervise(actor: &actor::Actor, tree: &Value, child: &mut Child) -> Result<(i32, &'static str)> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status.code().unwrap_or(1), "natural"));
        }
        actor::require(&tree["monitor"])?;
        if control(actor)?.is_some() {
            child.kill()?;
            return Ok((child.wait()?.code().unwrap_or(1), "forced"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
