use std::{
    net::{Ipv4Addr, TcpListener},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use crate::{
    Result,
    process::{ChildGroup, ManagedChild, stop_child},
    watch::SourceWatcher,
};

use super::{
    build::cleanup_binaries,
    command::{RuntimeInputPaths, api_command, worker_command},
    config::{DevPorts, WorkerIds},
    health::{
        available_ports, combine_failures, http_readyz, start_worker_after_api_ready,
        wait_healthy_until, wait_services_ready_until, wait_services_ready_until_controlled,
    },
    model::{Binaries, CycleControl, HEALTH_TIMEOUT, ProbeResult, ServiceLaunchMode, Services},
};

pub(super) fn probe_candidate(
    group: &ChildGroup,
    root: &Path,
    binaries: &Binaries,
    lkg: &mut Services,
    worker_ids: WorkerIds,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
) -> Result<ProbeResult> {
    let (api_port, worker_port) = available_ports()?;
    let mut command = api_command(
        root,
        &binaries.api,
        RuntimeInputPaths::new(&binaries.config_dir, &binaries.locales_dir),
        api_port,
        worker_port,
        worker_ids.probe_api,
        true,
    );
    let mut api = group.spawn(&mut command)?;
    let mut worker_command = worker_command(
        root,
        &binaries.worker,
        RuntimeInputPaths::new(&binaries.config_dir, &binaries.locales_dir),
        worker_port,
        worker_ids.probe_worker,
        true,
    );
    let mut worker = match group.spawn(&mut worker_command) {
        Ok(worker) => worker,
        Err(error) => {
            let api_stop = stop_child(&mut api);
            return combine_failures(Err(error), [("停止候选 API", api_stop)]);
        }
    };
    let health_deadline = Instant::now() + HEALTH_TIMEOUT;
    let result = wait_services_ready_until_controlled(
        health_deadline,
        "候选 API",
        "候选 Worker",
        || {
            ensure_running("候选 API", &mut api)?;
            ensure_running("last-known-good API", &mut lkg.api)
        },
        || {
            ensure_running("候选 Worker", &mut worker)?;
            ensure_running("last-known-good Worker", &mut lkg.worker)
        },
        || http_readyz(api_port),
        || http_readyz(worker_port),
        || {
            if shutdown.load(Ordering::Acquire) {
                CycleControl::Shutdown
            } else if watcher.is_superseded(binaries.source_revision) {
                CycleControl::Superseded
            } else {
                CycleControl::Continue
            }
        },
    );
    let api_stop = stop_child(&mut api);
    let worker_stop = stop_child(&mut worker);
    let control = combine_failures(
        result,
        [("停止候选 API", api_stop), ("停止候选 Worker", worker_stop)],
    )?;
    Ok(match control {
        CycleControl::Continue => ProbeResult::Ready,
        CycleControl::Superseded => ProbeResult::Superseded,
        CycleControl::Shutdown => ProbeResult::Cancelled,
    })
}

pub(super) fn start_services(
    group: &ChildGroup,
    root: &Path,
    binaries: Binaries,
    ports: DevPorts,
    worker_ids: WorkerIds,
) -> Result<Services> {
    start_services_in_mode(
        group,
        root,
        binaries,
        ports,
        worker_ids,
        ServiceLaunchMode::Development,
    )
}

pub(super) fn start_services_in_mode(
    group: &ChildGroup,
    root: &Path,
    binaries: Binaries,
    ports: DevPorts,
    worker_ids: WorkerIds,
    mode: ServiceLaunchMode,
) -> Result<Services> {
    let mut api_command = api_command(
        root,
        &binaries.api,
        RuntimeInputPaths::new(&binaries.config_dir, &binaries.locales_dir),
        ports.api,
        ports.worker,
        worker_ids.api,
        mode.is_probe(),
    );
    let mut api = match group.spawn(&mut api_command) {
        Ok(api) => api,
        Err(error) => {
            let cleanup = cleanup_binaries(&binaries);
            return combine_failures(Err(error), [("清理候选版本目录", cleanup)]);
        }
    };
    let health_deadline = Instant::now() + HEALTH_TIMEOUT;
    let mut worker_command = worker_command(
        root,
        &binaries.worker,
        RuntimeInputPaths::new(&binaries.config_dir, &binaries.locales_dir),
        ports.worker,
        worker_ids.worker,
        mode.is_probe(),
    );
    let mut worker = match start_worker_after_api_ready(
        || wait_healthy_until(&mut api, ports.api, "API", health_deadline),
        || group.spawn(&mut worker_command),
    ) {
        Ok(worker) => worker,
        Err(error) => {
            let api_stop = stop_child(&mut api);
            let cleanup = cleanup_binaries(&binaries);
            return combine_failures(
                Err(error),
                [
                    ("停止未完成启动的 API", api_stop),
                    ("清理候选版本目录", cleanup),
                ],
            );
        }
    };
    if let Err(error) = wait_services_ready_until(
        health_deadline,
        "API",
        "Worker",
        || ensure_running("API", &mut api),
        || ensure_running("Worker", &mut worker),
        || http_readyz(ports.api),
        || http_readyz(ports.worker),
    ) {
        let api_stop = stop_child(&mut api);
        let worker_stop = stop_child(&mut worker);
        let cleanup = cleanup_binaries(&binaries);
        return combine_failures(
            Err(error),
            [
                ("停止未通过健康检查的 API", api_stop),
                ("停止未通过健康检查的 Worker", worker_stop),
                ("清理候选版本目录", cleanup),
            ],
        );
    }
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
    ports: DevPorts,
    worker_ids: WorkerIds,
    mode: ServiceLaunchMode,
) -> Result<bool> {
    let previous = current.binaries.clone();
    let restore = previous.clone();
    switch_services_with_rollback(
        current,
        stop_services,
        || start_services_in_mode(group, root, candidate, ports, worker_ids, mode),
        || start_services_in_mode(group, root, restore, ports, worker_ids, mode),
        || {
            let _ = cleanup_binaries(&previous);
        },
    )
}

pub(crate) fn switch_services_with_rollback<T>(
    current: &mut T,
    stop_current: impl FnOnce(&mut T) -> Result<()>,
    start_candidate: impl FnOnce() -> Result<T>,
    restore_previous: impl FnOnce() -> Result<T>,
    on_promoted: impl FnOnce(),
) -> Result<bool> {
    if let Err(stop_error) = stop_current(current) {
        println!("停止当前服务失败，正在恢复 last-known-good：{stop_error}");
        return match restore_previous() {
            Ok(previous) => {
                *current = previous;
                Ok(false)
            }
            Err(restore_error) => Err(format!(
                "停止当前服务失败：{stop_error}；last-known-good 恢复失败：{restore_error}"
            )
            .into()),
        };
    }
    match start_candidate() {
        Ok(next) => {
            on_promoted();
            *current = next;
            Ok(true)
        }
        Err(candidate_error) => {
            println!("候选在正式端口启动失败，正在恢复 last-known-good：{candidate_error}");
            match restore_previous() {
                Ok(previous) => {
                    *current = previous;
                    Ok(false)
                }
                Err(restore_error) => Err(format!(
                    "候选在正式端口启动失败：{candidate_error}；last-known-good 恢复失败：{restore_error}"
                )
                .into()),
            }
        }
    }
}

pub(super) fn ensure_initial_ports_available(ports: DevPorts) -> Result<()> {
    for (label, port) in [("API", ports.api), ("Worker", ports.worker)] {
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| {
            format!("{label} 开发端口 {port} 已被占用，请停止旧进程或调整环境变量：{error}")
        })?;
    }
    Ok(())
}

pub(super) fn ensure_running(label: &str, child: &mut ManagedChild) -> Result<()> {
    if let Some(status) = child.try_wait()? {
        Err(format!("{label} 进程意外退出：{status}").into())
    } else {
        Ok(())
    }
}

pub(super) fn ensure_services_ready(services: &mut Services, ports: DevPorts) -> Result<()> {
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    wait_services_ready_until(
        deadline,
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
    ports: DevPorts,
    worker_ids: WorkerIds,
    mode: ServiceLaunchMode,
) -> Result<()> {
    let binaries = services.binaries.clone();
    stop_services(services)?;
    *services = start_services_in_mode(group, root, binaries, ports, worker_ids, mode)?;
    Ok(())
}

pub(super) fn stop_services(services: &mut Services) -> Result<()> {
    let api = stop_child(&mut services.api);
    let worker = stop_child(&mut services.worker);
    api?;
    worker
}

pub(super) fn stop_all(services: &mut Services, vite: &mut ManagedChild) -> Result<()> {
    let backend = stop_services(services);
    let frontend = stop_child(vite);
    backend?;
    frontend
}
