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
    watch::{ChangeBatch, SourceWatcher, WatchEvent},
    workspace::root_dir,
};

use super::{
    build::{build_candidate, cleanup_binaries},
    config::{DevPorts, WorkerIds, spawn_shutdown_listener},
    model::{
        Binaries, BuildPlan, BuildResult, LOOP_INTERVAL, ProbeResult, RunningProcesses,
        WATCH_DEBOUNCE,
    },
    services::{
        ensure_initial_ports_available, ensure_running, probe_candidate, promote_services,
        restore_last_known_good, start_services, stop_all,
    },
};

enum LoopControl {
    Continue,
    Shutdown,
}

pub(crate) fn run(frontend_dir: &Path) -> Result<()> {
    doctor::run(frontend_dir)?;
    let root = root_dir();
    let ports = DevPorts::from_environment()?;
    ensure_initial_ports_available(ports)?;
    let worker_ids = WorkerIds::from_environment()?;
    let group = ChildGroup::new()?;
    let watcher = SourceWatcher::new(&root)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let _shutdown_listener = spawn_shutdown_listener(Arc::clone(&shutdown));

    let proxy_target = format!("http://127.0.0.1:{}", ports.api);
    let mut vite = spawn_pnpm_with_env(
        &group,
        frontend_dir,
        &["dev"],
        &[("VITE_APP_PROXY_TARGET", proxy_target.as_str())],
    )?;
    println!("Vite 已启动；正在构建初始 API、Worker 与迁移工具。按 Ctrl+C 可统一停止。");
    let initial = match build_initial_candidate(&group, &root, &shutdown, &watcher) {
        Ok(Some(candidate)) => candidate,
        Ok(None) => {
            stop_child(&mut vite)?;
            return Ok(());
        }
        Err(error) => {
            let _ = stop_child(&mut vite);
            return Err(error);
        }
    };
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
    supervise(
        &group,
        &root,
        &watcher,
        &shutdown,
        &mut running,
        ports,
        worker_ids,
    )
}

fn build_initial_candidate(
    group: &ChildGroup,
    root: &Path,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
) -> Result<Option<Binaries>> {
    loop {
        let plan = BuildPlan::initial(watcher.current_revision());
        match build_candidate(group, root, shutdown, watcher, &plan, None, None)? {
            BuildResult::Ready(candidate) => return Ok(Some(candidate)),
            BuildResult::Superseded => {
                println!("初始候选已过期，立即按最新源码重新构建。");
            }
            BuildResult::Failed | BuildResult::VerifiedNoRestart => {
                return Err("初始后端构建失败，尚无 last-known-good 可继续运行".into());
            }
            BuildResult::Cancelled => return Ok(None),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn supervise(
    group: &ChildGroup,
    root: &Path,
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    running: &mut RunningProcesses,
    ports: DevPorts,
    worker_ids: WorkerIds,
) -> Result<()> {
    loop {
        if shutdown.load(Ordering::Acquire) {
            println!("收到中断信号，正在停止 API、Worker 与 Vite。");
            stop_all(&mut running.services, &mut running.vite)?;
            return Ok(());
        }
        ensure_running("API", &mut running.services.api)?;
        ensure_running("Worker", &mut running.services.worker)?;
        ensure_running("Vite", &mut running.vite)?;
        let Some(batch) = receive_change(watcher)? else {
            continue;
        };
        if matches!(
            process_change(
                group, root, watcher, shutdown, running, ports, worker_ids, batch
            )?,
            LoopControl::Shutdown
        ) {
            stop_all(&mut running.services, &mut running.vite)?;
            return Ok(());
        }
    }
}

fn receive_change(watcher: &SourceWatcher) -> Result<Option<ChangeBatch>> {
    let Some(event) = watcher.recv_timeout(LOOP_INTERVAL)? else {
        return Ok(None);
    };
    match event {
        WatchEvent::Failed(error) => Err(error.into()),
        WatchEvent::BackendChanged { revision, path } => watcher
            .drain_changes(revision, path, WATCH_DEBOUNCE)
            .map(Some),
    }
}

#[allow(clippy::too_many_arguments)]
fn process_change(
    group: &ChildGroup,
    root: &Path,
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    running: &mut RunningProcesses,
    ports: DevPorts,
    worker_ids: WorkerIds,
    batch: ChangeBatch,
) -> Result<LoopControl> {
    let plan = BuildPlan::from_changes(&batch);
    let changed_paths = batch.paths.iter().cloned().collect::<Vec<_>>().join(", ");
    println!(
        "检测到后端变更 r{} [{changed_paths}]，后台准备新候选；当前服务继续运行。",
        batch.revision.value()
    );
    if plan.tool_self_changed {
        return Err("xtask 自身已变化，请重新运行 `cargo dev`".into());
    }
    if batch.revision <= running.services.binaries.source_revision {
        println!("该变更已包含在当前 last-known-good 中，跳过重复构建。");
        return Ok(LoopControl::Continue);
    }
    if plan.is_noop() {
        println!("这些文件不影响当前 dev 运行时，已忽略。");
        return Ok(LoopControl::Continue);
    }
    let previous = running.services.binaries.clone();
    let result = {
        let mut lkg_check = || {
            ensure_running("last-known-good API", &mut running.services.api)?;
            ensure_running("last-known-good Worker", &mut running.services.worker)
        };
        build_candidate(
            group,
            root,
            shutdown,
            watcher,
            &plan,
            Some(&previous),
            Some(&mut lkg_check),
        )
    };
    let result = match result {
        Ok(result) => result,
        Err(error)
            if error
                .to_string()
                .starts_with("last-known-good 在候选阶段失效") =>
        {
            println!("{error}；正在恢复 last-known-good 配对。");
            restore_last_known_good(group, root, &mut running.services, ports, worker_ids)?;
            return Ok(LoopControl::Continue);
        }
        Err(error) => return Err(error),
    };
    handle_candidate_result(
        group, root, watcher, shutdown, running, ports, worker_ids, result,
    )
}

#[allow(clippy::too_many_arguments)]
fn handle_candidate_result(
    group: &ChildGroup,
    root: &Path,
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    running: &mut RunningProcesses,
    ports: DevPorts,
    worker_ids: WorkerIds,
    result: BuildResult,
) -> Result<LoopControl> {
    let candidate = match result {
        BuildResult::Failed => {
            println!("候选构建或迁移校验失败，继续使用 last-known-good。");
            return Ok(LoopControl::Continue);
        }
        BuildResult::Cancelled => return Ok(LoopControl::Shutdown),
        BuildResult::VerifiedNoRestart => {
            println!("变更已校验，不需要重启 API/Worker。");
            return Ok(LoopControl::Continue);
        }
        BuildResult::Superseded => {
            println!("候选已被更新的源码代次取代，立即规划下一轮。");
            return Ok(LoopControl::Continue);
        }
        BuildResult::Ready(candidate) => candidate,
    };
    match probe_candidate(
        group,
        root,
        &candidate,
        &mut running.services,
        worker_ids,
        shutdown,
        watcher,
    ) {
        Ok(ProbeResult::Ready) => {}
        Ok(ProbeResult::Superseded) => {
            println!("候选 probe 被更新源码取代，继续使用 last-known-good。");
            let _ = cleanup_binaries(root, &candidate);
            return Ok(LoopControl::Continue);
        }
        Ok(ProbeResult::Cancelled) => {
            let _ = cleanup_binaries(root, &candidate);
            return Ok(LoopControl::Shutdown);
        }
        Err(error) => {
            println!("候选 API/Worker 未通过隔离健康检查，继续使用 last-known-good：{error}");
            let _ = cleanup_binaries(root, &candidate);
            if ensure_running("last-known-good API", &mut running.services.api).is_err()
                || ensure_running("last-known-good Worker", &mut running.services.worker).is_err()
            {
                restore_last_known_good(group, root, &mut running.services, ports, worker_ids)?;
            }
            return Ok(LoopControl::Continue);
        }
    }
    if promote_services(
        group,
        root,
        &mut running.services,
        candidate,
        ports,
        worker_ids,
    )? {
        println!("新候选健康检查通过，API 与 Worker 已切换。");
    }
    Ok(LoopControl::Continue)
}
