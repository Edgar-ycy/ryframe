use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use crate::{
    Result, doctor,
    process::{ChildGroup, ManagedChild, spawn_pnpm_with_env, stop_child},
    watch::{ChangeBatch, SourceWatcher, WatchEvent},
    workspace::root_dir,
};

use super::{
    build::{BuildContext, build_candidate, cleanup_binaries},
    config::{DevPorts, WorkerIds, spawn_shutdown_listener},
    model::{
        Binaries, BuildPlan, BuildResult, ChangeOutcome, LOOP_INTERVAL, ProbeResult,
        RunningProcesses, ServiceLaunchMode, Services, WATCH_DEBOUNCE,
    },
    services::{
        ensure_initial_ports_available, ensure_running, ensure_services_ready, probe_candidate,
        promote_services, restore_last_known_good, start_services, stop_all,
    },
    snapshot::DevSession,
    tool_self_changed_error,
};

pub(crate) fn run(frontend_dir: &Path) -> Result<()> {
    doctor::run(frontend_dir)?;
    let root = root_dir();
    let ports = DevPorts::from_environment()?;
    ensure_initial_ports_available(ports)?;
    let worker_ids = WorkerIds::from_environment()?;
    let group = ChildGroup::new()?;
    let watcher = SourceWatcher::new(&root)?;
    let (session, recovered) = DevSession::prepare(&root, watcher.current_revision())?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let _shutdown_listener = spawn_shutdown_listener(Arc::clone(&shutdown));

    let proxy_target = format!("http://127.0.0.1:{}", ports.api);
    let vite = spawn_pnpm_with_env(
        &group,
        frontend_dir,
        &["dev"],
        &[("VITE_APP_PROXY_TARGET", proxy_target.as_str())],
    )?;
    run_with_vite(
        &group, &root, &session, &watcher, &shutdown, vite, ports, worker_ids, recovered,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_with_vite(
    group: &ChildGroup,
    root: &Path,
    session: &DevSession,
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    mut vite: ManagedChild,
    ports: DevPorts,
    worker_ids: WorkerIds,
    recovered: Option<Binaries>,
) -> Result<()> {
    let backend = match start_backend(
        group, root, session, shutdown, watcher, ports, worker_ids, recovered,
    ) {
        Ok(backend) => backend,
        Err(error) => {
            let _ = stop_child(&mut vite);
            return Err(error);
        }
    };
    let Some((services, recovered)) = backend else {
        stop_child(&mut vite)?;
        return Ok(());
    };
    let mut running = RunningProcesses { services, vite };
    if recovered {
        println!("已恢复最近完整的 last-known-good；正在后台构建当前源码候选。");
        if matches!(
            refresh_recovered(
                group,
                root,
                session,
                watcher,
                shutdown,
                &mut running,
                ports,
                worker_ids,
            )?,
            ChangeOutcome::Shutdown
        ) {
            stop_all(&mut running.services, &mut running.vite)?;
            return Ok(());
        }
    }
    println!(
        "开发服务已就绪：API http://127.0.0.1:{}/readyz，Worker http://127.0.0.1:{}/readyz。",
        ports.api, ports.worker
    );
    supervise(
        group,
        root,
        session,
        watcher,
        shutdown,
        &mut running,
        ports,
        worker_ids,
    )
}

#[allow(clippy::too_many_arguments)]
fn start_backend(
    group: &ChildGroup,
    root: &Path,
    session: &DevSession,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    ports: DevPorts,
    worker_ids: WorkerIds,
    recovered: Option<Binaries>,
) -> Result<Option<(Services, bool)>> {
    if let Some(binaries) = recovered {
        match start_services(group, root, binaries, ports, worker_ids) {
            Ok(services) => return Ok(Some((services, true))),
            Err(error) => {
                println!("恢复的 last-known-good 无法启动，改为构建当前源码：{error}");
            }
        }
    }
    println!("Vite 已启动；正在构建初始 API、Worker 与迁移工具。按 Ctrl+C 可统一停止。");
    let initial = match build_initial_candidate(group, root, session, shutdown, watcher)? {
        Some(candidate) => candidate,
        None => return Ok(None),
    };
    start_services(group, root, initial, ports, worker_ids).map(|services| Some((services, false)))
}

pub(super) fn build_initial_candidate(
    group: &ChildGroup,
    root: &Path,
    session: &DevSession,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
) -> Result<Option<Binaries>> {
    let context = BuildContext::new(group, root, session, shutdown, watcher);
    loop {
        let plan = BuildPlan::initial(watcher.current_revision());
        match build_candidate(&context, &plan, None, None)? {
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
    session: &DevSession,
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
                group,
                root,
                session,
                watcher,
                shutdown,
                &mut running.services,
                ports,
                worker_ids,
                ServiceLaunchMode::Development,
                None,
                batch,
            )?,
            ChangeOutcome::Shutdown
        ) {
            stop_all(&mut running.services, &mut running.vite)?;
            return Ok(());
        }
    }
}

pub(super) fn receive_change(watcher: &SourceWatcher) -> Result<Option<ChangeBatch>> {
    let Some(event) = watcher.recv_timeout(LOOP_INTERVAL)? else {
        return Ok(None);
    };
    match event {
        WatchEvent::Failed(error) => Err(error.into()),
        WatchEvent::BackendChanged { revision, paths } => watcher
            .drain_changes(revision, paths, WATCH_DEBOUNCE)
            .map(Some),
    }
}

#[allow(clippy::too_many_arguments)]
fn refresh_recovered(
    group: &ChildGroup,
    root: &Path,
    session: &DevSession,
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    running: &mut RunningProcesses,
    ports: DevPorts,
    worker_ids: WorkerIds,
) -> Result<ChangeOutcome> {
    let plan = BuildPlan::initial(watcher.current_revision());
    let Some(result) = build_while_lkg(
        group,
        root,
        session,
        watcher,
        shutdown,
        &mut running.services,
        ports,
        worker_ids,
        &plan,
        ServiceLaunchMode::Development,
        None,
    )?
    else {
        return Ok(ChangeOutcome::Failed);
    };
    handle_candidate_result(
        group,
        root,
        watcher,
        shutdown,
        &mut running.services,
        ports,
        worker_ids,
        ServiceLaunchMode::Development,
        result,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_while_lkg(
    group: &ChildGroup,
    root: &Path,
    session: &DevSession,
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    services: &mut Services,
    ports: DevPorts,
    worker_ids: WorkerIds,
    plan: &BuildPlan,
    mode: ServiceLaunchMode,
    cargo_invocations: Option<&AtomicUsize>,
) -> Result<Option<BuildResult>> {
    let previous = services.binaries.clone();
    let context = BuildContext::new(group, root, session, shutdown, watcher);
    let context = if let Some(counter) = cargo_invocations {
        context.with_cargo_counter(counter)
    } else {
        context
    };
    let result = {
        let mut lkg_check = || {
            ensure_running("last-known-good API", &mut services.api)?;
            ensure_running("last-known-good Worker", &mut services.worker)
        };
        build_candidate(&context, plan, Some(&previous), Some(&mut lkg_check))
    };
    match result {
        Ok(result) => Ok(Some(result)),
        Err(error)
            if error
                .to_string()
                .starts_with("last-known-good 在候选阶段失效") =>
        {
            println!("{error}；正在恢复 last-known-good 配对。");
            restore_last_known_good(group, root, services, ports, worker_ids, mode)?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn process_change(
    group: &ChildGroup,
    root: &Path,
    session: &DevSession,
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    services: &mut Services,
    ports: DevPorts,
    worker_ids: WorkerIds,
    mode: ServiceLaunchMode,
    cargo_invocations: Option<&AtomicUsize>,
    batch: ChangeBatch,
) -> Result<ChangeOutcome> {
    let plan = BuildPlan::from_changes(&batch);
    let changed_paths = batch.paths.iter().cloned().collect::<Vec<_>>().join(", ");
    println!(
        "检测到后端变更 r{} [{changed_paths}]，后台准备新候选；当前服务继续运行。",
        batch.revision.value()
    );
    if plan.tool_self_changed {
        return Err(tool_self_changed_error());
    }
    if batch.revision <= services.binaries.source_revision {
        println!("该变更已包含在当前 last-known-good 中，跳过重复构建。");
        return Ok(ChangeOutcome::Ignored);
    }
    if plan.is_noop() {
        println!("这些文件不影响当前 dev 运行时，已忽略。");
        return Ok(ChangeOutcome::Ignored);
    }
    let Some(result) = build_while_lkg(
        group,
        root,
        session,
        watcher,
        shutdown,
        services,
        ports,
        worker_ids,
        &plan,
        mode,
        cargo_invocations,
    )?
    else {
        return Ok(ChangeOutcome::Failed);
    };
    handle_candidate_result(
        group, root, watcher, shutdown, services, ports, worker_ids, mode, result,
    )
}

#[allow(clippy::too_many_arguments)]
fn handle_candidate_result(
    group: &ChildGroup,
    root: &Path,
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    services: &mut Services,
    ports: DevPorts,
    worker_ids: WorkerIds,
    mode: ServiceLaunchMode,
    result: BuildResult,
) -> Result<ChangeOutcome> {
    let candidate = match result {
        BuildResult::Failed => {
            println!("候选构建或迁移校验失败，继续使用 last-known-good。");
            return Ok(ChangeOutcome::Failed);
        }
        BuildResult::Cancelled => return Ok(ChangeOutcome::Shutdown),
        BuildResult::VerifiedNoRestart => {
            if let Err(error) = ensure_services_ready(services, ports) {
                println!("变更校验完成，但 last-known-good 不再就绪，正在恢复配对：{error}");
                restore_last_known_good(group, root, services, ports, worker_ids, mode)?;
                return Ok(ChangeOutcome::Failed);
            }
            println!("变更已校验，不需要重启 API/Worker。");
            return Ok(ChangeOutcome::VerifiedNoRestart);
        }
        BuildResult::Superseded => {
            println!("候选已被更新的源码代次取代，立即规划下一轮。");
            return Ok(ChangeOutcome::Superseded);
        }
        BuildResult::Ready(candidate) => candidate,
    };
    match probe_candidate(
        group, root, &candidate, services, worker_ids, shutdown, watcher,
    ) {
        Ok(ProbeResult::Ready) => {}
        Ok(ProbeResult::Superseded) => {
            println!("候选 probe 被更新源码取代，继续使用 last-known-good。");
            let _ = cleanup_binaries(&candidate);
            return Ok(ChangeOutcome::Superseded);
        }
        Ok(ProbeResult::Cancelled) => {
            let _ = cleanup_binaries(&candidate);
            return Ok(ChangeOutcome::Shutdown);
        }
        Err(error) => {
            println!("候选 API/Worker 未通过隔离健康检查，继续使用 last-known-good：{error}");
            let _ = cleanup_binaries(&candidate);
            if ensure_running("last-known-good API", &mut services.api).is_err()
                || ensure_running("last-known-good Worker", &mut services.worker).is_err()
            {
                restore_last_known_good(group, root, services, ports, worker_ids, mode)?;
            }
            return Ok(ChangeOutcome::Failed);
        }
    }
    if shutdown.load(Ordering::Acquire) {
        let _ = cleanup_binaries(&candidate);
        return Ok(ChangeOutcome::Shutdown);
    }
    if !watcher.final_revision_fence(candidate.source_revision) {
        println!("候选在正式切换前被更新源码取代，继续使用 last-known-good。");
        let _ = cleanup_binaries(&candidate);
        return Ok(ChangeOutcome::Superseded);
    }
    if promote_services(group, root, services, candidate, ports, worker_ids, mode)? {
        println!("新候选健康检查通过，API 与 Worker 已切换。");
        Ok(ChangeOutcome::Promoted)
    } else {
        Ok(ChangeOutcome::Failed)
    }
}
