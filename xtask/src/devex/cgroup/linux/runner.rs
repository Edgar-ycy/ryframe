use std::{
    ffi::OsString,
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use nix::unistd::{Gid, Uid, getegid, geteuid, getgid, getuid, setgid, setgroups, setuid};
use serde::Serialize;

use crate::workspace::root_dir;

use super::super::{
    CGROUP_ROOT, CgroupPlan,
    error::{GateError, GateResult},
    evidence::{KernelIdentity, read_json, write_json_new},
    plan::{cargo_command, validate_plan_at, validate_test_output},
    processes::{
        ProcessFact, ProcessRegistry, append_registry, create_registry, register_children,
        registry_path,
    },
    resolve_output_at,
};
use super::{cgroup_fs, owned_root};

const CARGO_TIMEOUT: Duration = Duration::from_secs(900);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Serialize)]
struct TestStart<'a> {
    command: &'a [String],
    uid: u32,
    gid: u32,
    launcher: &'a ProcessFact,
    cgroup: PathBuf,
    root_identity: &'a KernelIdentity,
}

pub(in crate::devex::cgroup) fn run_cli(arguments: &[OsString]) -> GateResult<()> {
    if arguments.len() != 2
        || arguments
            .first()
            .and_then(|value| value.to_str())
            .is_none_or(|value| value != "--output")
    {
        return Err(GateError::new(
            "runner",
            "内部 cgroup runner 参数必须是 --output <目录>",
        ));
    }
    let supplied = PathBuf::from(&arguments[1]);
    let output = resolve_output_at(&supplied, &root_dir())
        .map_err(|error| GateError::new("runner", error.to_string()))?;
    let plan: CgroupPlan = read_json(&output.join("plan.json"))?;
    validate_plan_at(&plan, &root_dir(), Path::new(CGROUP_ROOT))?;
    if geteuid().as_raw() != 0 {
        return Err(GateError::new(
            "runner",
            "Cargo 降权跳板必须由 root test 阶段启动",
        ));
    }
    run(&plan, &output)
}

fn run(plan: &CgroupPlan, output: &Path) -> GateResult<()> {
    let created = owned_root(plan, output)?;
    join_driver(&plan.root.join("driver"))?;
    drop_privileges(plan)?;
    let launcher = cgroup_fs::process_fact(std::process::id())?
        .ok_or_else(|| GateError::new("runner", "无法登记 Cargo 降权跳板的内核创建身份"))?;
    let registry_path = registry_path(output);
    create_registry(&registry_path, &launcher)?;
    let command = cargo_command(plan);
    write_json_new(
        &output.join("test-start.json"),
        &TestStart {
            command: &command,
            uid: getuid().as_raw(),
            gid: getgid().as_raw(),
            launcher: &launcher,
            cgroup: plan.root.join("driver"),
            root_identity: &created.identity,
        },
    )?;
    let mut registry = ProcessRegistry::from([(launcher.pid, launcher)]);
    run_cargo(plan, output, &command, &registry_path, &mut registry)
}

fn join_driver(driver: &Path) -> GateResult<()> {
    let pid = std::process::id().to_string();
    cgroup_fs::write_control(&driver.join("cgroup.procs"), "0")?;
    let members = std::fs::read_to_string(driver.join("cgroup.procs")).map_err(|error| {
        GateError::io(
            "runner",
            "回读 driver cgroup 成员",
            &driver.join("cgroup.procs"),
            &error,
        )
    })?;
    if !members.lines().any(|member| member.trim() == pid) {
        return Err(GateError::new(
            "runner",
            format!(
                "Cargo 跳板 PID {pid} 未进入 {}；拒绝在错误 cgroup 中启动测试",
                driver.display()
            ),
        ));
    }
    Ok(())
}

fn drop_privileges(plan: &CgroupPlan) -> GateResult<()> {
    setgroups(&[]).map_err(|error| {
        GateError::new(
            "privilege",
            format!("无法清空辅助组，拒绝启动 Cargo：{error}"),
        )
    })?;
    setgid(Gid::from_raw(plan.gid)).map_err(|error| {
        GateError::new(
            "privilege",
            format!("无法把 Cargo 跳板切换到 gid {}：{error}", plan.gid),
        )
    })?;
    setuid(Uid::from_raw(plan.uid)).map_err(|error| {
        GateError::new(
            "privilege",
            format!("无法把 Cargo 跳板切换到 uid {}：{error}", plan.uid),
        )
    })?;
    let actual = (
        getuid().as_raw(),
        geteuid().as_raw(),
        getgid().as_raw(),
        getegid().as_raw(),
    );
    if actual != (plan.uid, plan.uid, plan.gid, plan.gid) {
        return Err(GateError::new(
            "privilege",
            format!(
                "Cargo 启动前未永久降权：要求 uid={} gid={}，实际 uid={} euid={} gid={} egid={}",
                plan.uid, plan.gid, actual.0, actual.1, actual.2, actual.3
            ),
        ));
    }
    Ok(())
}

fn run_cargo(
    plan: &CgroupPlan,
    output: &Path,
    command: &[String],
    registry_path: &Path,
    registry: &mut ProcessRegistry,
) -> GateResult<()> {
    let log_path = output.join("cargo.log");
    let log = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&log_path)
        .map_err(|error| GateError::io("test", "创建 Cargo 验收日志", &log_path, &error))?;
    let stderr = log
        .try_clone()
        .map_err(|error| GateError::io("test", "复制 Cargo 验收日志", &log_path, &error))?;
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .current_dir(&plan.backend)
        .env_clear()
        .envs(&plan.environment)
        .env("CARGO_INCREMENTAL", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    let mut child = process.spawn().map_err(|error| {
        GateError::new(
            "test",
            format!(
                "无法以 uid {} gid {} 启动 `{}`：{error}",
                plan.uid,
                plan.gid,
                command.join(" ")
            ),
        )
    })?;
    if let Some(fact) = cgroup_fs::process_fact(child.id())? {
        registry.insert(fact.pid, fact.clone());
        append_registry(registry_path, &[fact])?;
    }
    let deadline = Instant::now() + CARGO_TIMEOUT;
    loop {
        if child
            .try_wait()
            .map_err(|error| GateError::new("test", format!("等待 Cargo 进程失败：{error}")))?
            .is_some()
        {
            break;
        }
        let current = cgroup_fs::processes(&plan.root)?;
        let additions = register_children(&current, registry, plan.uid)?;
        append_registry(registry_path, &additions)?;
        if Instant::now() >= deadline {
            return Err(GateError::new(
                "timeout",
                "定向 cgroup Cargo 测试超过 15 分钟；保留证据，随后由独立 cleanup 阶段精确冻结并回收 cgroup",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
    let current = cgroup_fs::processes(&plan.root)?;
    let additions = register_children(&current, registry, plan.uid)?;
    append_registry(registry_path, &additions)?;
    let code = child
        .wait()
        .map_err(|error| GateError::new("test", format!("读取 Cargo 退出状态失败：{error}")))?
        .code()
        .ok_or_else(|| GateError::new("test", "Cargo 被信号终止且没有可用退出码"))?;
    validate_cargo_result(plan, output, code)
}

fn validate_cargo_result(plan: &CgroupPlan, output: &Path, code: i32) -> GateResult<()> {
    let log_path = output.join("cargo.log");
    let text = std::fs::read(&log_path)
        .map_err(|error| GateError::io("test", "读取 Cargo 验收日志", &log_path, &error))?;
    validate_test_output(code, &String::from_utf8_lossy(&text)).map_err(|error| {
        GateError::new(
            "test",
            format!("{error}；完整 Cargo 日志：{}", log_path.display()),
        )
    })?;
    let measure = plan.root.join("measure");
    let leftovers = std::fs::read_dir(&measure)
        .map_err(|error| GateError::io("test", "扫描测量 cgroup", &measure, &error))?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        })
        .collect::<Vec<_>>();
    if !leftovers.is_empty() {
        return Err(GateError::new(
            "test",
            format!(
                "Rust 测试后测量子树未清理：{}",
                leftovers
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("、")
            ),
        ));
    }
    let log = OpenOptions::new()
        .append(true)
        .open(&log_path)
        .map_err(|error| GateError::io("test", "复核 Cargo 验收日志", &log_path, &error))?;
    log.sync_all()
        .map_err(|error| GateError::io("test", "刷新 Cargo 验收日志", &log_path, &error))?;
    Ok(())
}
