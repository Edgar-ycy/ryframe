use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    Result, doctor,
    process::{ChildGroup, spawn_pnpm_with_env, stop_child},
    watch::{SourceWatcher, WatchEvent},
    workspace::root_dir,
};

use super::{
    build::{build_candidate, cleanup_binaries},
    config::{DevPorts, WorkerIds, spawn_shutdown_listener},
    model::{BuildResult, LOOP_INTERVAL, RunningProcesses, WATCH_DEBOUNCE},
    services::{
        ensure_initial_ports_available, ensure_running, probe_candidate, promote_services,
        start_services, stop_all,
    },
};

pub(crate) fn run(frontend_dir: &Path) -> Result<()> {
    doctor::run(frontend_dir)?;
    let root = root_dir();
    let ports = DevPorts::from_environment()?;
    ensure_initial_ports_available(ports)?;
    let worker_ids = WorkerIds::from_environment()?;
    let group = ChildGroup::new()?;
    let watcher = SourceWatcher::new(&root)?;
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let _shutdown_listener = spawn_shutdown_listener(Arc::clone(&shutdown_requested));

    let proxy_target = format!("http://127.0.0.1:{}", ports.api);
    let mut vite = spawn_pnpm_with_env(
        &group,
        frontend_dir,
        &["dev"],
        &[("VITE_APP_PROXY_TARGET", proxy_target.as_str())],
    )?;
    println!("Vite 已启动；正在构建 API、Worker 与迁移工具。按 Ctrl+C 可统一停止。");
    let initial = match build_candidate(&group, &root, &shutdown_requested)? {
        BuildResult::Ready(candidate) => candidate,
        BuildResult::Failed => {
            stop_child(&mut vite)?;
            return Err("初始后端构建失败，尚无 last-known-good 可继续运行".into());
        }
        BuildResult::Cancelled => {
            stop_child(&mut vite)?;
            return Ok(());
        }
    };
    // 首次启动没有 last-known-good，必须由正常模式完成依赖 provision；只读 probe 仅用于后续热切换。
    let services = match start_services(&group, &root, initial, ports, worker_ids) {
        Ok(services) => services,
        Err(error) => {
            let _ = stop_child(&mut vite);
            return Err(error);
        }
    };
    let mut running = RunningProcesses { services, vite };
    println!(
        "开发服务已就绪：API http://127.0.0.1:{}/readyz，Worker http://127.0.0.1:{}/readyz。",
        ports.api, ports.worker
    );

    loop {
        if shutdown_requested.load(Ordering::Acquire) {
            println!("收到中断信号，正在停止 API、Worker 与 Vite。");
            stop_all(&mut running.services, &mut running.vite)?;
            return Ok(());
        }
        ensure_running("API", &mut running.services.api)?;
        ensure_running("Worker", &mut running.services.worker)?;
        ensure_running("Vite", &mut running.vite)?;

        let Some(event) = watcher.recv_timeout(LOOP_INTERVAL)? else {
            continue;
        };
        let event = match event {
            WatchEvent::Failed(error) => return Err(error.into()),
            WatchEvent::BackendChanged(path) => watcher.drain_changes(path, WATCH_DEBOUNCE)?,
        };
        let changed_path = match event {
            WatchEvent::BackendChanged(path) => path,
            WatchEvent::Failed(error) => return Err(error.into()),
        };
        println!("检测到后端变更 {changed_path}，后台编译新候选；当前服务继续运行。");
        match build_candidate(&group, &root, &shutdown_requested)? {
            BuildResult::Failed => {
                println!("候选构建或迁移校验失败，继续使用 last-known-good。");
            }
            BuildResult::Cancelled => {
                stop_all(&mut running.services, &mut running.vite)?;
                return Ok(());
            }
            BuildResult::Ready(candidate) => {
                if let Err(error) = probe_candidate(&group, &root, &candidate, worker_ids) {
                    println!(
                        "候选 API/Worker 未通过隔离健康检查，继续使用 last-known-good：{error}"
                    );
                    let _ = cleanup_binaries(&root, &candidate);
                    continue;
                }
                if promote_services(
                    &group,
                    &root,
                    &mut running.services,
                    candidate,
                    ports,
                    worker_ids,
                )? {
                    println!("新候选健康检查通过，API 与 Worker 已切换。");
                }
            }
        }
    }
}
