use super::{
    build::cleanup_binaries,
    command::{RuntimeInputPaths, api_command, worker_command},
    config::DevPorts,
    health::{
        combine_failures, http_readyz, start_worker_after_api_ready, wait_healthy_until,
        wait_probe_services_ready_until_controlled, wait_services_ready_until,
    },
    inputs,
    model::{Binaries, CycleControl, HEALTH_TIMEOUT, ProbeResult, ServiceLaunchMode, Services},
};
use crate::{
    Result,
    process::{ChildGroup, ManagedChild, stop_child},
    watch::SourceWatcher,
};
use std::{
    fmt,
    net::{Ipv4Addr, TcpListener},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

#[derive(Debug)]
pub(super) struct UncertainProcesses(String);
impl fmt::Display for UncertainProcesses {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for UncertainProcesses {}
pub(super) fn uncertain(error: &(dyn std::error::Error + 'static)) -> bool {
    error.is::<UncertainProcesses>()
}

fn stopped_result<T>(result: Result<T>, stops: [(&str, Result<()>); 2]) -> Result<T> {
    let uncertain = stops.iter().any(|(_, value)| value.is_err());
    let result = combine_failures(result, stops);
    if uncertain {
        return result.map_err(|error| {
            Box::new(UncertainProcesses(format!(
                "{error}；停止状态不确定，保留全部代次现场，拒绝重复启动 Worker"
            ))) as Box<dyn std::error::Error>
        });
    }
    result
}

pub(super) fn probe_candidate(
    group: &ChildGroup,
    root: &Path,
    binaries: &Binaries,
    lkg: &mut Services,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
) -> Result<ProbeResult> {
    let ports = inputs::load_ports(&binaries.runtime_dir, ServiceLaunchMode::Probe)?;
    let api_config = inputs::role_config(&binaries.runtime_dir, ServiceLaunchMode::Probe, false);
    let mut api = group.spawn(api_command(
        root,
        &binaries.api,
        RuntimeInputPaths::new(&api_config),
        true,
    ))?;
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    let mut control = || {
        if shutdown.load(Ordering::Acquire) {
            CycleControl::Shutdown
        } else if watcher.is_superseded(binaries.source_revision) {
            CycleControl::Superseded
        } else {
            CycleControl::Continue
        }
    };
    let api_ready = super::health::wait_probe_api_until_controlled(
        deadline,
        ports.api,
        || {
            ensure_running("候选 API", &mut api)?;
            ensure_running("last-known-good API", &mut lkg.api)?;
            ensure_running("last-known-good Worker", &mut lkg.worker)
        },
        &mut control,
    );
    if !matches!(api_ready, Ok(CycleControl::Continue)) {
        return stopped_result(
            api_ready.map(probe_control),
            [
                ("停止候选 API", stop_child(&mut api)),
                ("候选 Worker 尚未启动", Ok(())),
            ],
        );
    }
    let worker_config = inputs::role_config(&binaries.runtime_dir, ServiceLaunchMode::Probe, true);
    let mut worker = match group.spawn(worker_command(
        root,
        &binaries.worker,
        RuntimeInputPaths::new(&worker_config),
        true,
    )) {
        Ok(worker) => worker,
        Err(error) => {
            return stopped_result(
                Err(error),
                [
                    ("停止候选 API", stop_child(&mut api)),
                    ("候选 Worker 尚未启动", Ok(())),
                ],
            );
        }
    };
    finish_probe(&mut api, &mut worker, lkg, ports, deadline, &mut control)
}

fn finish_probe(
    api: &mut ManagedChild,
    worker: &mut ManagedChild,
    lkg: &mut Services,
    ports: DevPorts,
    deadline: Instant,
    mut control: impl FnMut() -> CycleControl,
) -> Result<ProbeResult> {
    let result = wait_probe_services_ready_until_controlled(
        deadline,
        "候选 API",
        "候选 Worker",
        || {
            ensure_running("候选 API", api)?;
            ensure_running("last-known-good API", &mut lkg.api)
        },
        || {
            ensure_running("候选 Worker", worker)?;
            ensure_running("last-known-good Worker", &mut lkg.worker)
        },
        ports.api,
        ports.worker,
        &mut control,
    );
    let stops = [
        ("停止候选 API", stop_child(api)),
        ("停止候选 Worker", stop_child(worker)),
    ];
    stopped_result(result.map(probe_control), stops)
}

fn probe_control(control: CycleControl) -> ProbeResult {
    match control {
        CycleControl::Continue => ProbeResult::Ready,
        CycleControl::Superseded => ProbeResult::Superseded,
        CycleControl::Shutdown => ProbeResult::Cancelled,
    }
}

pub(super) fn start_services(
    group: &ChildGroup,
    root: &Path,
    binaries: Binaries,
) -> Result<Services> {
    start_services_in_mode(group, root, binaries, ServiceLaunchMode::Development)
}

pub(super) fn start_services_in_mode(
    group: &ChildGroup,
    root: &Path,
    binaries: Binaries,
    mode: ServiceLaunchMode,
) -> Result<Services> {
    let ports = inputs::load_ports(&binaries.runtime_dir, mode)?;
    let api_config = inputs::role_config(&binaries.runtime_dir, mode, false);
    let worker_config = inputs::role_config(&binaries.runtime_dir, mode, true);
    let mut api = group.spawn(api_command(
        root,
        &binaries.api,
        RuntimeInputPaths::new(&api_config),
        mode.is_probe(),
    ))?;
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    let worker = start_worker_after_api_ready(
        || wait_healthy_until(&mut api, ports.api, "API", deadline),
        || {
            group.spawn(worker_command(
                root,
                &binaries.worker,
                RuntimeInputPaths::new(&worker_config),
                mode.is_probe(),
            ))
        },
    );
    let mut worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            return stopped_result(
                Err(error),
                [
                    ("停止未完成启动的 API", stop_child(&mut api)),
                    ("Worker 尚未启动", Ok(())),
                ],
            );
        }
    };
    let ready = wait_services_ready_until(
        deadline,
        "API",
        "Worker",
        || ensure_running("API", &mut api),
        || ensure_running("Worker", &mut worker),
        || http_readyz(ports.api),
        || http_readyz(ports.worker),
    );
    if let Err(error) = ready {
        return stopped_result(
            Err(error),
            [
                ("停止未就绪 API", stop_child(&mut api)),
                ("停止未就绪 Worker", stop_child(&mut worker)),
            ],
        );
    }
    // 启动函数不拥有清理权；尤其恢复 LKG 启动失败时不得删除旧快照。
    Ok(Services {
        api,
        worker,
        binaries,
    })
}

pub(super) fn promote_services(
    group: &ChildGroup,
    root: &Path,
    current: &mut Services,
    candidate: Binaries,
    mode: ServiceLaunchMode,
) -> Result<bool> {
    let previous = current.binaries.clone();
    let restore = previous.clone();
    let failed_candidate = candidate.clone();
    let promoted = switch_services_with_rollback(
        current,
        stop_services,
        || start_services_in_mode(group, root, candidate, mode),
        || {
            super::snapshot::verify_binaries(&restore)?;
            start_services_in_mode(group, root, restore, mode)
        },
        || {
            let _ = cleanup_binaries(&previous);
        },
    )?;
    if !promoted {
        cleanup_binaries(&failed_candidate)?;
    }
    Ok(promoted)
}

pub(crate) fn switch_services_with_rollback<T>(
    current: &mut T,
    stop_current: impl FnOnce(&mut T) -> Result<()>,
    start_candidate: impl FnOnce() -> Result<T>,
    restore_previous: impl FnOnce() -> Result<T>,
    on_promoted: impl FnOnce(),
) -> Result<bool> {
    // 部分停止或未知身份不允许再启动另一 Worker；保留现场交由调用方报告。
    stop_current(current)?;
    match start_candidate() {
        Ok(next) => {
            on_promoted();
            *current = next;
            Ok(true)
        }
        Err(error) if uncertain(error.as_ref()) => Err(error),
        Err(candidate_error) => {
            println!("候选正式启动失败：{candidate_error}；正在恢复 last-known-good 完整代次。");
            match restore_previous() {
                Ok(previous) => {
                    *current = previous;
                    println!("last-known-good 回滚成功，已恢复旧代次配置、端口和实例编号。");
                    Ok(false)
                }
                Err(restore_error) => Err(format!("候选启动失败：{candidate_error}；last-known-good 恢复失败：{restore_error}；完整代次现场已保留").into()),
            }
        }
    }
}

pub(super) fn ensure_initial_ports_available(ports: DevPorts) -> Result<()> {
    for (label, port) in [("API", ports.api), ("Worker", ports.worker)] {
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| {
            format!(
                "{label} 开发端口 {port} 已占用：{error}；请停止已确认归属的进程或修改配置文件端口"
            )
        })?;
    }
    Ok(())
}
pub(super) fn ensure_running(label: &str, child: &mut ManagedChild) -> Result<()> {
    if let Some(status) = child.try_wait()? {
        return Err(format!("{label} 进程意外退出：{status}").into());
    }
    Ok(())
}
pub(super) fn ensure_services_ready(
    services: &mut Services,
    mode: ServiceLaunchMode,
) -> Result<()> {
    let ports = inputs::load_ports(&services.binaries.runtime_dir, mode)?;
    wait_services_ready_until(
        Instant::now() + HEALTH_TIMEOUT,
        "last-known-good API",
        "last-known-good Worker",
        || ensure_running("last-known-good API", &mut services.api),
        || ensure_running("last-known-good Worker", &mut services.worker),
        || http_readyz(ports.api),
        || http_readyz(ports.worker),
    )
}
pub(super) fn restore_last_known_good(
    group: &ChildGroup,
    root: &Path,
    services: &mut Services,
    mode: ServiceLaunchMode,
) -> Result<()> {
    let binaries = services.binaries.clone();
    stop_services(services)?;
    super::snapshot::verify_binaries(&binaries)?;
    *services = start_services_in_mode(group, root, binaries, mode)?;
    Ok(())
}
pub(super) fn stop_services(services: &mut Services) -> Result<()> {
    stopped_result(
        Ok(()),
        [
            ("停止 API", stop_child(&mut services.api)),
            ("停止 Worker", stop_child(&mut services.worker)),
        ],
    )
}
pub(super) fn stop_all(services: &mut Services) -> Result<()> {
    stop_services(services)
}
