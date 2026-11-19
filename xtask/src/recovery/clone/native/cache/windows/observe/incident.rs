//! 接受已登记的事故失败证据，区分重启证明和原 Job 退出收据。
use super::{
    Generation, LAUNCH, RECEIPTS, Registered, Result, Value, common, directory_files, document,
    entry_exists, json, present, prove_external,
};
use crate::recovery::source::evidence::reboot_process_exited;
use std::path::Path;

const FAILURE: &str = "failed.json";

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
    let registered =
        Registered::open_registered(backend, Some(request), runtime, Generation::IncidentReboot)?;
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
        Ok(json!({
            "kind":"windows-redis-reboot-incident",
            "failure":registered.incident_failure,
            "runtime":common::binding(&registered.launch.output.join("runtime.json"))?,
            "supervisor":supervisor, "process":process, "port":request["port"],
            "normal_job_exit_proof":null
        }))
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
