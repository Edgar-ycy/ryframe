//! 持久持有 Redis 的 Windows Job，只对本次授权、创建身份和退出签发收据。
use super::{BoundLaunch, exact_arguments, publish};
use crate::{
    Result,
    process::{ChildGroup, ManagedChild, stop_child},
    recovery::{
        clone::native::{json as document, lifecycle::identity, resources::common},
        fixture_control::services::lifecycle::readonly::process,
    },
};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use winsafe::{HPROCESS, co, guard::CloseHandleGuard, prelude::kernel_Hprocess};

pub(super) fn serve(launch: &BoundLaunch) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let supervisor = identity::current()?;
    let mut child = None;
    let mut members = Members::default();
    let mut spawn_attempted = false;
    let mut process_binding = None;
    let outcome: Result<End> = (|| {
        launch.check()?;
        let _protected = launch.protect()?;
        let authorization = json!({"launch":launch.binding,"supervisor":supervisor});
        wait_authorized(launch, &authorization, deadline)?;
        let command = redis_command(launch)?;
        let group = ChildGroup::new()?;
        launch.check()?;
        if read_control(&launch.output.join("authorized.json"))?.as_ref() != Some(&authorization) {
            return Err("Windows Redis 启动授权在创建前变化".into());
        }
        starting(launch, deadline)?;
        spawn_attempted = true;
        let product = child.insert(group.spawn(command)?);
        let (registered, binding) = register(launch, &supervisor, product, &mut members, deadline)?;
        let binding = process_binding.insert(binding);
        wait_accepted(
            launch,
            product,
            &mut members,
            &registered,
            binding,
            deadline,
        )?;
        supervise(launch, product, &mut members)
    })();
    let (cleanup_completed, result, cancelled) = match outcome {
        Ok(ended) => {
            let (returncode, cancelled) = match ended {
                End::Natural(returncode) => (returncode, false),
                End::Cancelled(returncode) => (returncode, true),
            };
            let published = finish_exit(
                launch,
                child.as_mut(),
                &mut members,
                process_binding.as_ref(),
                returncode,
            );
            (published.is_ok(), published, cancelled)
        }
        Err(error) => {
            let completed = if spawn_attempted {
                finish_exit(
                    launch,
                    child.as_mut(),
                    &mut members,
                    process_binding.as_ref(),
                    None,
                )
                .is_ok()
            } else {
                child.is_none()
                    && process_binding.is_none()
                    && matches!(
                        fs::symlink_metadata(launch.output.join("process.json")),
                        Err(error) if error.kind() == ErrorKind::NotFound
                    )
            };
            (completed, Err(error), false)
        }
    };
    if result.is_ok() && !cancelled {
        return result;
    }
    let failure = publish(
        &launch.output.join("failed.json"),
        &json!({"launch":launch.binding,"supervisor":supervisor,
            "spawn_attempted":spawn_attempted,"cleanup_completed":cleanup_completed}),
    );
    // 保留原操作错误；真实撤销成功后也必须完整发布失败收据。
    result.and(failure)
}

fn read_control(path: &Path) -> Result<Option<Value>> {
    match fs::symlink_metadata(path) {
        Ok(_) => document::read(path).map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn cancelled(launch: &BoundLaunch) -> Result<bool> {
    let Some(receipt) = read_control(&launch.output.join("cancel.json"))? else {
        return Ok(false);
    };
    if receipt != launch.binding {
        return Err("Windows Redis 取消收据不属于原启动绑定".into());
    }
    Ok(true)
}

fn starting(launch: &BoundLaunch, deadline: Instant) -> Result<()> {
    before_deadline(deadline)?;
    if cancelled(launch)? {
        return Err("Windows Redis 启动在接受前取消".into());
    }
    process::require(&launch.owner)?;
    before_deadline(deadline)
}

fn before_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err("Windows Redis 启动监督期限耗尽".into());
    }
    Ok(())
}

fn pause(deadline: Instant) {
    thread::sleep(
        Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
    );
}

fn wait_authorized(launch: &BoundLaunch, authorization: &Value, deadline: Instant) -> Result<()> {
    let path = launch.output.join("authorized.json");
    loop {
        starting(launch, deadline)?;
        if let Some(value) = read_control(&path)? {
            if &value != authorization {
                return Err("Windows Redis 启动授权不属于原绑定与监督器".into());
            }
            return before_deadline(deadline);
        }
        pause(deadline);
    }
}

fn redis_command(launch: &BoundLaunch) -> Result<Command> {
    let tool = Path::new(document::text(
        &launch.request["tool"]["path"],
        "Windows Redis 工具",
    )?);
    let directory = Path::new(document::text(
        &launch.request["directory"]["path"],
        "Windows Redis 数据目录",
    )?);
    if !tool.is_absolute() || !directory.is_absolute() {
        return Err("Windows Redis 工具及工作目录必须为固定绝对路径".into());
    }
    let mut command = Command::new(tool);
    command
        .arg("redis.conf")
        .current_dir(directory)
        .env_clear()
        .stdin(Stdio::null());
    for (key, value) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if ["SYSTEMROOT", "WINDIR", "SYSTEMDRIVE", "TEMP", "TMP"]
            .iter()
            .any(|allowed| name.eq_ignore_ascii_case(allowed))
        {
            command.env(key, value);
        }
    }
    let log = |name| {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(launch.output.join(name))
    };
    command
        .stdout(log("redis.stdout.log")?)
        .stderr(log("redis.stderr.log")?);
    Ok(command)
}

fn register(
    launch: &BoundLaunch,
    supervisor: &Value,
    product: &mut ManagedChild,
    members: &mut Members,
    deadline: Instant,
) -> Result<(Value, Value)> {
    let pid = product.id();
    let registered = common::process_identity(pid)?.ok_or("Windows Redis 创建身份缺失")?;
    require_product(launch, product, pid, &registered)?;
    members.capture(product)?;
    if !members.0.iter().any(|member| member.identity == registered) {
        return Err("Windows Redis 缺少实际进程成员句柄".into());
    }
    launch.check()?;
    starting(launch, deadline)?;
    if product.try_wait()?.is_some() {
        return Err("Windows Redis 进程在登记前已退出".into());
    }
    before_deadline(deadline)?;
    let receipt = json!({"launch":launch.binding,"supervisor":supervisor,"process":registered});
    let path = launch.output.join("process.json");
    publish(&path, &receipt)?;
    let binding = common::binding(&path)?;
    if document::read(&process_path(launch, &binding)?)? != receipt {
        return Err("Windows Redis 进程收据在登记期间变化".into());
    }
    Ok((registered, binding))
}

fn require_product(
    launch: &BoundLaunch,
    child: &mut ManagedChild,
    pid: u32,
    registered: &Value,
) -> Result<()> {
    let tool = document::text(&launch.request["tool"]["path"], "Windows Redis 工具")?;
    if registered["pid"].as_u64() != Some(u64::from(pid))
        || child.try_wait()?.is_some()
        || !child.member_pids()?.contains(&pid)
    {
        return Err("Windows Redis 原 Job 成员身份无效".into());
    }
    exact_arguments(registered, tool, &["redis.conf"])?;
    if child.try_wait()?.is_some() || !child.member_pids()?.contains(&pid) {
        return Err("Windows Redis 观察参数后原 Job 成员变化".into());
    }
    Ok(())
}

fn process_path(launch: &BoundLaunch, binding: &Value) -> Result<std::path::PathBuf> {
    let path = common::bound_file(&launch.backend, binding, "Windows Redis 原进程收据")?;
    if path != launch.output.join("process.json") {
        return Err("Windows Redis 原进程收据路径变化".into());
    }
    Ok(path)
}

fn wait_accepted(
    launch: &BoundLaunch,
    child: &mut ManagedChild,
    members: &mut Members,
    registered: &Value,
    binding: &Value,
    deadline: Instant,
) -> Result<()> {
    let pid = child.id();
    let path = launch.output.join("acknowledged.json");
    loop {
        starting(launch, deadline)?;
        if child.try_wait()?.is_some() {
            return Err("Windows Redis 进程在接受前已退出".into());
        }
        members.capture(child)?;
        let Some(ack) = read_control(&path)? else {
            pause(deadline);
            continue;
        };
        document::exact(&ack, &["process", "run_id"], "Windows Redis 确认")?;
        if ack["process"] != *binding
            || !ack["run_id"]
                .as_str()
                .is_some_and(|id| crate::frontend_origin::manifest::hex_digest(id, 40))
        {
            return Err("Windows Redis 确认未绑定原进程收据或 run_id 无效".into());
        }
        launch.check()?;
        process_path(launch, binding)?;
        require_product(launch, child, pid, registered)?;
        if read_control(&path)?.as_ref() != Some(&ack) {
            return Err("Windows Redis 确认在接受前变化".into());
        }
        starting(launch, deadline)?;
        if child.try_wait()?.is_some() {
            return Err("Windows Redis 进程在发布接受前已退出".into());
        }
        before_deadline(deadline)?;
        publish(&launch.output.join("accepted.json"), &ack)?;
        return before_deadline(deadline);
    }
}

enum End {
    Natural(Option<i32>),
    Cancelled(Option<i32>),
}

fn supervise(launch: &BoundLaunch, child: &mut ManagedChild, members: &mut Members) -> Result<End> {
    loop {
        if cancelled(launch)? {
            launch.check()?;
            return exited_code(child, members).map(End::Cancelled);
        }
        if child.try_wait()?.is_some() {
            return exited_code(child, members).map(End::Natural);
        }
        members.capture(child)?;
        thread::sleep(Duration::from_millis(100));
    }
}

fn stop_owned(child: &mut ManagedChild, members: &mut Members) -> Result<i32> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let observed = members.capture(child);
    // 成员采集失败仍仅回收自有 Job；本分支只调用一次 stop_child。
    let stopped = stop_child(child);
    observed?;
    stopped?;
    loop {
        if let Some(returncode) = exited_code(child, members)? {
            return Ok(returncode);
        }
        if Instant::now() >= deadline {
            return Err("Windows Redis 原 Job 或成员句柄未确认完整退出".into());
        }
        pause(deadline);
    }
}

fn exited_code(child: &mut ManagedChild, members: &Members) -> Result<Option<i32>> {
    let Some(status) = child.try_wait()? else {
        return Ok(None);
    };
    if child.active_process_count()? != Some(0)
        || !child.member_pids()?.is_empty()
        || !members.all_exited()?
    {
        return Ok(None);
    }
    let code = status.code().ok_or("Windows Redis 原始进程退出码未确认")?;
    Ok(Some(code))
}

struct Member {
    identity: Value,
    handle: CloseHandleGuard<HPROCESS>,
}

#[derive(Default)]
struct Members(Vec<Member>);

impl Members {
    fn capture(&mut self, child: &ManagedChild) -> Result<()> {
        for pid in child.member_pids()? {
            let Some(observed) = common::process_identity(pid)? else {
                continue;
            };
            if self.0.iter().any(|member| member.identity == observed) {
                continue;
            }
            let handle = match HPROCESS::OpenProcess(
                co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
                false,
                pid,
            ) {
                Ok(handle) => handle,
                Err(co::ERROR::INVALID_PARAMETER) => continue,
                Err(error) => return Err(error.into()),
            };
            let captured = identity::windows_snapshot(&handle, pid);
            if handle.WaitForSingleObject(Some(0))? == co::WAIT::OBJECT_0 {
                continue;
            }
            if captured? != observed || !child.member_pids()?.contains(&pid) {
                return Err("Windows Redis Job 成员创建身份在读取期间变化".into());
            }
            self.0.push(Member {
                identity: observed,
                handle,
            });
        }
        Ok(())
    }

    fn all_exited(&self) -> Result<bool> {
        for member in &self.0 {
            if member.handle.WaitForSingleObject(Some(0))? != co::WAIT::OBJECT_0 {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn publish_exit(launch: &BoundLaunch, binding: &Value, returncode: i32) -> Result<()> {
    process_path(launch, binding)?;
    publish(
        &launch.output.join("exited.json"),
        &json!({"process":binding,"returncode":returncode,"job_empty":true}),
    )
}

fn finish_exit(
    launch: &BoundLaunch,
    child: Option<&mut ManagedChild>,
    members: &mut Members,
    binding: Option<&Value>,
    returncode: Option<i32>,
) -> Result<()> {
    let returncode = match returncode {
        Some(returncode) => returncode,
        None => stop_owned(child.ok_or("Windows Redis 缺少原 Job 句柄")?, members)?,
    };
    publish_exit(
        launch,
        binding.ok_or("Windows Redis 缺少原进程收据")?,
        returncode,
    )
}
