//! 接受已登记的事故失败证据，区分重启证明和原 Job 退出收据。
use super::{
    Generation, LAUNCH, RECEIPTS, Registered, Result, Value, common, directory_files, document,
    entry_exists, json, present, prove_external, same_path,
};
use crate::recovery::{
    clone::native::lifecycle::identity,
    source::evidence::{host_boot_proof, reboot_process_exited, verify_host_boot_proof},
};
use std::path::Path;

const FAILURE: &str = "failed.json";
const BOOT_DRIFT_TICKS: u64 = 60 * 10_000_000;

pub(in crate::recovery::clone::native::cache::windows) fn incomplete_cleanup(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    live: bool,
) -> Result<Value> {
    let registered = Registered::open_registered(
        backend,
        Some(request),
        runtime,
        Generation::Incident { frozen: !live },
    )?;
    let observed = (|| {
        if !same_path(
            document::text(&registered.launch.owner["executable"], "原控制器映像")?,
            document::text(&registered.launch.coordinator["path"], "原协调工具")?,
        ) {
            return Err("Windows Redis 事故原控制器与原协调工具不同".into());
        }
        if live {
            let identities = [&runtime["supervisor"], &runtime["process"]];
            let before = same_boot(&registered.launch.owner, identities)?;
            if !prove_external(registered.exited, identities, strict_present, || {
                registered.check_closed_port()
            })? {
                return Err("Windows Redis 事故缺少原身份退出和原端口释放观察".into());
            }
            let after = same_boot(&registered.launch.owner, identities)?;
            if before.abs_diff(after) > BOOT_DRIFT_TICKS {
                return Err("Windows Redis 事故核验期间当前主机启动刻度变化".into());
            }
        }
        incident_receipt(&registered, "windows-redis-incomplete-cleanup-incident")
    })();
    registered.finish(observed)
}

fn same_boot(owner: &Value, [supervisor, process]: [&Value; 2]) -> Result<u64> {
    let proof = host_boot_proof()?;
    let boot = verify_host_boot_proof(&proof)?;
    // 创建刻度紧贴启动边界时无法排除时钟漂移，按事故既有启动观察余量判定。
    let lower = boot
        .checked_add(BOOT_DRIFT_TICKS)
        .ok_or("Windows Redis 事故启动刻度溢出")?;
    let upper = proof["observed_ticks"]
        .as_u64()
        .ok_or("Windows Redis 事故缺少主机采集刻度")?;
    for value in [owner, supervisor, process] {
        require_started_in_window(value, lower, upper)?;
    }
    Ok(boot)
}

fn require_started_in_window(value: &Value, lower: u64, upper: u64) -> Result<()> {
    let time =
        document::text(&value["started"], "Windows Redis 事故进程创建刻度")?.parse::<u64>()?;
    if time <= lower || time > upper {
        return Err("Windows Redis 原身份没有处于本次主机启动之后".into());
    }
    Ok(())
}

fn strict_present(expected: &Value) -> Result<bool> {
    use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};

    let pid = expected["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or("Windows Redis 事故进程 PID 无效")?;
    let handle = match HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    ) {
        Ok(handle) => handle,
        Err(co::ERROR::INVALID_PARAMETER) => return Ok(false),
        Err(error) => {
            return Err(format!("无法核对 Windows Redis 事故进程 {pid}：{error}").into());
        }
    };
    // 即使句柄已经发出退出信号，可打开的 PID 也不能作为事故不存在证明。
    if identity::windows_snapshot(&handle, pid)? != *expected {
        return Err("Windows Redis 事故 PID 已复用，保留现场".into());
    }
    Ok(true)
}

fn incident_receipt(registered: &Registered<'_>, kind: &str) -> Result<Value> {
    let failure = registered
        .incident_failure
        .as_ref()
        .ok_or("Windows Redis 缺少原事故失败绑定")?;
    Ok(json!({
        "kind": kind,
        "failure": failure,
        "runtime": common::binding(&registered.launch.output.join("runtime.json"))?,
        "supervisor": registered.runtime["supervisor"],
        "process": registered.runtime["process"],
        "port": registered.launch.request["port"],
        "normal_job_exit_proof": null
    }))
}

/// 复核原 Redis 的事故和当前物理状态，不能作为正常停止或启动收据。
pub(in crate::recovery) fn redis_reboot_incident(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    boot: u64,
) -> Result<Value> {
    observe(backend, request, runtime, boot, true)
}

/// 重读历史事故证据，不将已经交给后继的端口或 PID 当作前驱状态。
pub(in crate::recovery) fn redis_reboot_evidence(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    boot: u64,
) -> Result<Value> {
    observe(backend, request, runtime, boot, false)
}

fn observe(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    boot: u64,
    live: bool,
) -> Result<Value> {
    let registered = Registered::open_registered(
        backend,
        Some(request),
        runtime,
        Generation::Incident { frozen: true },
    )?;
    let observed = (|| {
        let supervisor = &runtime["supervisor"];
        let process = &runtime["process"];
        // 分别校验两份 FILETIME；任何身份都不能从任一 PID 的消失推断。
        let ended = [
            reboot_process_exited(supervisor, boot)?,
            reboot_process_exited(process, boot)?,
        ];
        if ended != [true, true] {
            return Err("Windows Redis 原身份必须早于主机重启安全余量".into());
        }
        if live
            && !prove_external(registered.exited, [supervisor, process], present, || {
                registered.check_closed_port()
            })?
        {
            return Err("Windows Redis 事故缺少原身份退出和原端口释放观察".into());
        }
        incident_receipt(&registered, "windows-redis-reboot-incident")
    })();
    registered.finish(observed)
}

impl Registered<'_> {
    pub(super) fn check_terminal(&self) -> Result<()> {
        let output = &self.launch.output;
        let failure = output.join(FAILURE);
        let incident = self.incident_failure.as_ref();
        if let Some(binding) = incident {
            if self.exited || entry_exists(&output.join("cancel.json"))? {
                return Err("Windows Redis 事故不能混用原退出或取消收据".into());
            }
            if common::binding(&failure)? != *binding {
                return Err("Windows Redis 事故失败收据在观察期间变化".into());
            }
            check_failure(
                &document::read(&failure)?,
                &self.launch.binding,
                &self.runtime["supervisor"],
            )?;
        } else {
            for name in ["cancel.json", FAILURE] {
                if entry_exists(&output.join(name))? {
                    return Err("Windows Redis 含取消或失败收据，禁止按正常代次处理".into());
                }
            }
        }
        let files = directory_files(output)?;
        if files.iter().any(|name| !known(name, incident.is_some())) {
            return Err("Windows Redis 登记目录含未知文件".into());
        }
        if files.iter().any(|name| name == "exited.json") != self.exited {
            return Err("Windows Redis 退出收据在观察窗口内变化".into());
        }
        Ok(())
    }
}

fn known(name: &str, incident: bool) -> bool {
    RECEIPTS.contains(&name)
        || [
            LAUNCH,
            "request.json",
            "intent.json",
            "redis.stdout.log",
            "redis.stderr.log",
        ]
        .contains(&name)
        || if incident {
            name == FAILURE
        } else {
            name == "exited.json"
        }
}

fn check_failure(failure: &Value, launch: &Value, supervisor: &Value) -> Result<()> {
    if *failure
        != json!({"launch":launch,"supervisor":supervisor,
            "spawn_attempted":true,"cleanup_completed":false})
    {
        return Err("Windows Redis 事故失败必须绑定原启动和原监督器，并保留未确认清理".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_cleanup_requires_a_process_filetime_in_the_current_boot() {
        let identity = |started: &str| json!({"started":started});
        assert!(require_started_in_window(&identity("11"), 10, 20).is_ok());
        assert!(require_started_in_window(&identity("20"), 10, 20).is_ok());
        for started in ["0", "9", "10", "21", "invalid"] {
            assert!(require_started_in_window(&identity(started), 10, 20).is_err());
        }
    }

    #[test]
    fn incomplete_cleanup_never_classifies_an_openable_pid_as_missing() {
        let mut current = identity::current().unwrap();
        assert!(strict_present(&current).unwrap());
        current["started"] = json!("1");
        assert!(strict_present(&current).is_err());
    }

    #[test]
    fn accepts_only_the_exact_unclean_failure_of_the_registered_supervisor() {
        let launch = json!({"path":"launch-request.json","sha256":"original","bytes":3});
        let supervisor = json!({"pid":1,"started":"2","executable":"supervisor.exe"});
        let before = json!({"launch":launch,"supervisor":supervisor,
            "spawn_attempted":true,"cleanup_completed":false});
        check_failure(&before, &launch, &supervisor).unwrap();
        for (field, value) in [
            ("launch", json!({"path":"unrelated.json"})),
            (
                "supervisor",
                json!({"pid":1,"started":"3","executable":"supervisor.exe"}),
            ),
            ("spawn_attempted", json!(false)),
            ("cleanup_completed", json!(true)),
            ("cleanup_completed", json!("false")),
            ("exited", json!(true)),
        ] {
            let mut bad = before.clone();
            bad[field] = value;
            assert!(check_failure(&bad, &launch, &supervisor).is_err());
        }
        for name in [
            "launch",
            "supervisor",
            "spawn_attempted",
            "cleanup_completed",
        ] {
            let mut bad = before.clone();
            bad.as_object_mut().unwrap().remove(name);
            assert!(check_failure(&bad, &launch, &supervisor).is_err());
        }
    }

    #[test]
    fn incident_terminal_file_is_exclusive_with_normal_exit_and_cancel() {
        assert!(known(FAILURE, true));
        assert!(!known(FAILURE, false));
        assert!(known("exited.json", false));
        assert!(!known("exited.json", true));
        for name in [
            "cancel.json",
            "FAILED.JSON",
            "failed.json.pending",
            "foreign.json",
        ] {
            assert!(!known(name, true));
            assert!(!known(name, false));
        }
    }
}
