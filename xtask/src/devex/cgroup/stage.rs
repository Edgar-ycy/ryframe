use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use nix::unistd::geteuid;

use crate::workspace::root_dir;

use super::{
    CGROUP_ROOT, CgroupPlan, STAGE_MARKER,
    error::{GateError, GateResult},
    evidence::{CgroupPhase, path_absent, read_json, write_failure, write_phase_receipt},
    linux,
    plan::{privileged_identity_matches, validate_plan_at},
    resolve_output_at,
};

const PREPARE_OR_CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);
const TEST_TIMEOUT: Duration = Duration::from_secs(960);

pub(super) fn run_cli(arguments: &[OsString]) -> GateResult<()> {
    if arguments.len() != 3
        || arguments
            .get(1)
            .and_then(|value| value.to_str())
            .is_none_or(|value| value != "--output")
    {
        return Err(GateError::new(
            "stage",
            "内部 cgroup 阶段参数必须是 <prepare|test|cleanup> --output <目录>",
        ));
    }
    let phase = parse_phase(&arguments[0])?;
    let supplied = PathBuf::from(&arguments[2]);
    let output = resolve_output_at(&supplied, &root_dir())
        .map_err(|error| GateError::new("stage", error.to_string()))?;
    run_privileged(phase, &output)
}

pub(super) fn sudo_phase(plan: &CgroupPlan, output: &Path, phase: CgroupPhase) -> GateResult<()> {
    let log_path = output.join(format!(
        "{}-controller-{}.log",
        phase.as_str(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut log = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&log_path)
        .map_err(|error| GateError::io("stage", "创建 cgroup 阶段日志", &log_path, &error))?;
    writeln!(
        log,
        "phase={} run_id={} attempt={}",
        phase.as_str(),
        plan.run_id,
        plan.attempt
    )
    .and_then(|()| log.flush())
    .map_err(|error| GateError::io("stage", "写入 cgroup 阶段日志", &log_path, &error))?;
    let stdout = log
        .try_clone()
        .map_err(|error| GateError::io("stage", "复制 cgroup 阶段日志句柄", &log_path, &error))?;
    let stderr = log
        .try_clone()
        .map_err(|error| GateError::io("stage", "复制 cgroup 阶段日志句柄", &log_path, &error))?;

    let executable = std::env::current_exe().map_err(|error| {
        GateError::new(
            "stage",
            format!("无法定位当前 xtask 可执行文件以启动 cgroup 阶段：{error}"),
        )
    })?;
    let mut command = Command::new("sudo");
    command
        .args(["-n"])
        .arg(executable)
        .arg(STAGE_MARKER)
        .arg(phase.as_str())
        .arg("--output")
        .arg(output)
        .current_dir(&plan.backend)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut child = command
        .spawn()
        .map_err(|error| {
            GateError::new(
                "stage",
                format!(
                    "无法启动 cgroup {} 阶段的 sudo 控制器：{error}；请确认 runner 具有无密码 sudo 权限并重新执行",
                    phase.as_str()
                ),
            )
        })?;
    let timeout = if phase == CgroupPhase::Test {
        TEST_TIMEOUT
    } else {
        PREPARE_OR_CLEANUP_TIMEOUT
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            GateError::new(
                "stage",
                format!("等待 cgroup {} 阶段失败：{error}", phase.as_str()),
            )
        })? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_process_group(child.id()).map_err(|error| {
                GateError::new(
                    "stage",
                    format!(
                        "cgroup {} 阶段超过 {} 秒且无法回收 sudo 控制器进程树：{error}",
                        phase.as_str(),
                        timeout.as_secs()
                    ),
                )
            })?;
            let _ = child.wait();
            return Err(GateError::new(
                "timeout",
                format!(
                    "cgroup {} 阶段超过 {} 秒；已终止控制器进程树，日志位于 {}，随后应执行 cleanup 精确回收本次 cgroup",
                    phase.as_str(),
                    timeout.as_secs(),
                    log_path.display()
                ),
            ));
        }
        thread::sleep(Duration::from_millis(50));
    };
    if status.success() {
        return Ok(());
    }
    let tail = fs::read_to_string(&log_path)
        .unwrap_or_default()
        .lines()
        .rev()
        .take(40)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Err(GateError::new(
        "stage",
        format!(
            "cgroup {} 阶段失败（{}）；sudo 日志：{}\n{}",
            phase.as_str(),
            status,
            log_path.display(),
            tail
        ),
    ))
}

fn terminate_process_group(pid: u32) -> GateResult<()> {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, kill},
        unistd::Pid,
    };

    let pid = i32::try_from(pid)
        .map_err(|_| GateError::new("stage", "cgroup 控制器 PID 超出 POSIX 范围"))?;
    match kill(Pid::from_raw(-pid), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(GateError::new(
            "stage",
            format!("无法终止 cgroup 控制器进程组 {pid}：{error}"),
        )),
    }
}

fn run_privileged(phase: CgroupPhase, output: &Path) -> GateResult<()> {
    let mut plan = None;
    let result = (|| {
        let parsed: CgroupPlan = read_json(&output.join("plan.json"))?;
        validate_plan_at(&parsed, &root_dir(), Path::new(CGROUP_ROOT))?;
        require_privilege(&parsed)?;
        plan = Some(parsed.clone());
        match phase {
            CgroupPhase::Prepare => {
                reject_existing_phase_receipt(phase, output)?;
                let result = linux::prepare(&parsed, output)?;
                write_phase_receipt(phase, &parsed, output, &result)
            }
            CgroupPhase::Test => {
                reject_existing_phase_receipt(phase, output)?;
                let result = linux::test(&parsed, output)?;
                write_phase_receipt(phase, &parsed, output, &result)
            }
            CgroupPhase::Cleanup => {
                let result = linux::cleanup(&parsed, output)?;
                if path_absent(&output.join("cleanup.json"))? {
                    write_phase_receipt(phase, &parsed, output, &result)?;
                }
                if !result.terminated_registered_processes.is_empty() {
                    return Err(GateError::new(
                        "process",
                        format!(
                            "测试结束仍有 {} 个已登记后代；已精确回收 cgroup，但本次验收必须失败",
                            result.terminated_registered_processes.len()
                        ),
                    ));
                }
                Ok(())
            }
        }
    })();
    match result {
        Ok(()) => Ok(()),
        Err(error) => match write_failure(phase, output, plan.as_ref(), &error) {
            Ok(_) => Err(error),
            Err(receipt_error) => Err(GateError::new(
                error.kind(),
                format!(
                    "{error}；另外无法写入 {} 阶段失败收据：{receipt_error}",
                    phase.as_str()
                ),
            )),
        },
    }
}

fn require_privilege(plan: &CgroupPlan) -> GateResult<()> {
    if geteuid().as_raw() != 0 {
        return Err(GateError::new(
            "privilege",
            "内部 cgroup 阶段必须由 sudo 以 root 启动；请通过公开入口重新执行",
        ));
    }
    let sudo_uid = std::env::var("SUDO_UID").unwrap_or_default();
    let sudo_gid = std::env::var("SUDO_GID").unwrap_or_default();
    if !privileged_identity_matches(plan, &sudo_uid, &sudo_gid) {
        return Err(GateError::new(
            "privilege",
            format!(
                "sudo 来源身份与计划不一致：要求 uid={} gid={}，实际 SUDO_UID={sudo_uid} SUDO_GID={sudo_gid}",
                plan.uid, plan.gid
            ),
        ));
    }
    Ok(())
}

fn reject_existing_phase_receipt(phase: CgroupPhase, output: &Path) -> GateResult<()> {
    let path = output.join(format!("{}.json", phase.as_str()));
    if path_absent(&path)? {
        Ok(())
    } else {
        Err(GateError::new(
            "evidence",
            format!(
                "{} 阶段收据 {} 已存在；拒绝覆盖或重复执行该阶段",
                phase.as_str(),
                path.display()
            ),
        ))
    }
}

fn parse_phase(value: &OsString) -> GateResult<CgroupPhase> {
    match value.to_str() {
        Some("prepare") => Ok(CgroupPhase::Prepare),
        Some("test") => Ok(CgroupPhase::Test),
        Some("cleanup") => Ok(CgroupPhase::Cleanup),
        _ => Err(GateError::new(
            "stage",
            "内部 cgroup 阶段只允许 prepare、test 或 cleanup",
        )),
    }
}
