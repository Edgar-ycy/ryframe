use super::{
    build::{BuildContext, build_candidate, cleanup_binaries},
    config::spawn_shutdown_listener,
    inputs,
    model::{
        Binaries, BuildPlan, BuildResult, ChangeOutcome, LOOP_INTERVAL, ProbeResult,
        RunningProcesses, ServiceLaunchMode, Services, WATCH_DEBOUNCE,
    },
    services::{
        ensure_initial_ports_available, ensure_running, ensure_services_ready, probe_candidate,
        promote_services, restore_last_known_good, start_services, stop_all, uncertain,
    },
    snapshot::{DevSession, recovery_seed},
    tool_self_changed_error,
};
use crate::{
    Result,
    cli::DevOptions,
    doctor,
    process::ChildGroup,
    watch::{ChangeBatch, SourceWatcher, WatchEvent},
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateProbeDisposition {
    Promote,
    KeepLastKnownGood,
    Superseded,
    Shutdown,
}
pub(crate) fn candidate_probe_disposition(
    result: &Result<ProbeResult>,
) -> CandidateProbeDisposition {
    match result {
        Ok(ProbeResult::Ready) => CandidateProbeDisposition::Promote,
        Ok(ProbeResult::Superseded) => CandidateProbeDisposition::Superseded,
        Ok(ProbeResult::Cancelled) => CandidateProbeDisposition::Shutdown,
        Err(_) => CandidateProbeDisposition::KeepLastKnownGood,
    }
}

pub(super) struct Cycle<'a> {
    pub(super) group: &'a ChildGroup,
    pub(super) root: &'a Path,
    pub(super) session: &'a DevSession,
    pub(super) shutdown: &'a AtomicBool,
    pub(super) mode: ServiceLaunchMode,
}

pub(crate) fn run(options: &DevOptions) -> Result<()> {
    let root = inputs::workspace(options)?;
    doctor::run_backend_at(&root)?;
    let (session, recovered) = DevSession::prepare(&root, options)?;
    let (source, _) = inputs::selected(&root, options.config_dir.as_deref())?;
    let roots = if let Some(saved) = &recovered {
        inputs::watch_roots(&root, &saved.runtime_config, source)
    } else {
        session.capture()?.1
    };
    let mut watcher =
        SourceWatcher::new_with_inputs(&root, roots, recovery_seed(recovered.as_ref())?)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let _listener = spawn_shutdown_listener(Arc::clone(&shutdown));
    let group = ChildGroup::new()?;
    let cycle = Cycle {
        group: &group,
        root: &root,
        session: &session,
        shutdown: &shutdown,
        mode: ServiceLaunchMode::Development,
    };
    let services = if let Some(saved) = recovered {
        // 恢复失败直接报告，保留旧二进制；未知停止状态不得转而启动第二个 Worker。
        start_services(&group, &root, saved)?
    } else {
        let Some(initial) = build_initial_candidate(&cycle, &mut watcher)? else {
            return Ok(());
        };
        let ports = inputs::load_ports(&initial.runtime_dir, ServiceLaunchMode::Development)?;
        ensure_initial_ports_available(ports)?;
        start_services(&group, &root, initial)?
    };
    let mut running = RunningProcesses { services };
    if running.services.binaries.source_revision < watcher.current_revision()
        && refresh_recovered(&cycle, &mut watcher, &mut running.services)?
            == ChangeOutcome::Shutdown
    {
        return stop_all(&mut running.services);
    }
    let ports = inputs::load_ports(&running.services.binaries.runtime_dir, cycle.mode)?;
    println!(
        "开发服务已就绪：API http://127.0.0.1:{}/readyz，Worker http://127.0.0.1:{}/readyz。",
        ports.api, ports.worker
    );
    supervise(&cycle, &mut watcher, &mut running)
}

fn rebind(cycle: &Cycle<'_>, watcher: &mut SourceWatcher) -> Result<()> {
    let (_, roots, _) = cycle.session.capture()?;
    watcher.update_inputs(roots)
}

pub(super) fn build_initial_candidate(
    cycle: &Cycle<'_>,
    watcher: &mut SourceWatcher,
) -> Result<Option<Binaries>> {
    loop {
        rebind(cycle, watcher)?;
        let Some(batch) = settle_change(
            watcher,
            cycle.shutdown,
            ChangeBatch {
                revision: watcher.current_revision(),
                paths: Default::default(),
            },
        )?
        else {
            return Ok(None);
        };
        if BuildPlan::from_changes(&batch).tool_self_changed {
            return Err(tool_self_changed_error());
        }
        let plan = BuildPlan::initial(batch.revision);
        let context = BuildContext::new(
            cycle.group,
            cycle.root,
            cycle.session,
            cycle.shutdown,
            watcher,
            Path::new("cargo"),
        );
        match build_candidate(&context, &plan, None, None)? {
            BuildResult::Ready(candidate) => {
                if watcher.final_revision_fence(candidate.source_revision) {
                    return Ok(Some(*candidate));
                }
                cleanup_binaries(&candidate)?;
            }
            BuildResult::Superseded => continue,
            BuildResult::Cancelled => return Ok(None),
            BuildResult::Failed | BuildResult::VerifiedNoRestart => {
                return Err("初始后端构建失败，尚无 last-known-good 可继续运行".into());
            }
        }
    }
}

fn supervise(
    cycle: &Cycle<'_>,
    watcher: &mut SourceWatcher,
    running: &mut RunningProcesses,
) -> Result<()> {
    loop {
        if cycle.shutdown.load(Ordering::Acquire) {
            return stop_all(&mut running.services);
        }
        ensure_running("API", &mut running.services.api)?;
        ensure_running("Worker", &mut running.services.worker)?;
        if let Some(batch) = receive_change(watcher)?
            && process_change(cycle, watcher, &mut running.services, None, batch)?
                == ChangeOutcome::Shutdown
        {
            return stop_all(&mut running.services);
        }
    }
}

pub(super) fn receive_change(watcher: &SourceWatcher) -> Result<Option<ChangeBatch>> {
    match watcher.recv_timeout(LOOP_INTERVAL)? {
        None => Ok(None),
        Some(WatchEvent::Failed(error)) => Err(error.into()),
        Some(WatchEvent::BackendChanged { revision, paths }) => watcher
            .drain_changes(revision, paths, WATCH_DEBOUNCE)
            .map(Some),
    }
}

pub(super) fn process_change(
    cycle: &Cycle<'_>,
    watcher: &mut SourceWatcher,
    services: &mut Services,
    counter: Option<&AtomicUsize>,
    mut batch: ChangeBatch,
) -> Result<ChangeOutcome> {
    loop {
        let Some(settled) = settle_change(watcher, cycle.shutdown, batch)? else {
            return Ok(ChangeOutcome::Shutdown);
        };
        batch = settled;
        let plan = BuildPlan::from_changes(&batch);
        if plan.tool_self_changed {
            return Err(tool_self_changed_error());
        }
        if batch.revision <= services.binaries.source_revision || plan.is_noop() {
            return Ok(ChangeOutcome::Ignored);
        }
        let (captured, roots, _) = match cycle.session.capture() {
            Ok(captured) => captured,
            Err(error) => {
                println!("配置候选无效，保留 LKG：{error}");
                return Ok(ChangeOutcome::Failed);
            }
        };
        watcher.update_inputs(roots)?;
        let Some(settled) = settle_change(watcher, cycle.shutdown, batch)? else {
            return Ok(ChangeOutcome::Shutdown);
        };
        batch = settled;
        if batch.revision != plan.source_revision {
            continue;
        }
        if plan.is_runtime_config_only()
            && captured.semantically_matches(&services.binaries.runtime_config)?
        {
            ensure_services_ready(services, cycle.mode)?;
            return Ok(ChangeOutcome::VerifiedNoRestart);
        }
        let outcome = run_plan(cycle, watcher, services, &plan, counter)?;
        // pending raw 即使最终被过滤也可能保守取消候选；保留整批路径重新规划。
        if outcome != ChangeOutcome::Superseded {
            return Ok(outcome);
        }
    }
}

fn settle_change(
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    mut batch: ChangeBatch,
) -> Result<Option<ChangeBatch>> {
    loop {
        if shutdown.load(Ordering::Acquire) {
            return Ok(None);
        }
        batch = watcher.drain_changes(batch.revision, batch.paths, LOOP_INTERVAL)?;
        if watcher.final_revision_fence(batch.revision) {
            return Ok(Some(batch));
        }
    }
}

fn refresh_recovered(
    cycle: &Cycle<'_>,
    watcher: &mut SourceWatcher,
    services: &mut Services,
) -> Result<ChangeOutcome> {
    loop {
        if let Err(error) = rebind(cycle, watcher) {
            println!("当前输入无效，保留已恢复 LKG：{error}");
            return Ok(ChangeOutcome::Failed);
        }
        let Some(batch) = settle_change(
            watcher,
            cycle.shutdown,
            ChangeBatch {
                revision: watcher.current_revision(),
                paths: Default::default(),
            },
        )?
        else {
            return Ok(ChangeOutcome::Shutdown);
        };
        if BuildPlan::from_changes(&batch).tool_self_changed {
            return Err(tool_self_changed_error());
        }
        let outcome = run_plan(
            cycle,
            watcher,
            services,
            &BuildPlan::initial(batch.revision),
            None,
        )?;
        if outcome != ChangeOutcome::Superseded {
            return Ok(outcome);
        }
    }
}

fn run_plan(
    cycle: &Cycle<'_>,
    watcher: &mut SourceWatcher,
    services: &mut Services,
    plan: &BuildPlan,
    counter: Option<&AtomicUsize>,
) -> Result<ChangeOutcome> {
    let previous = services.binaries.clone();
    let mut context = BuildContext::new(
        cycle.group,
        cycle.root,
        cycle.session,
        cycle.shutdown,
        watcher,
        Path::new("cargo"),
    );
    if let Some(counter) = counter {
        context = context.with_cargo_counter(counter);
    }
    let result = {
        let mut lkg = || {
            ensure_running("last-known-good API", &mut services.api)?;
            ensure_running("last-known-good Worker", &mut services.worker)
        };
        build_candidate(&context, plan, Some(&previous), Some(&mut lkg))
    };
    let result = match result {
        Ok(result) => result,
        Err(error)
            if error
                .to_string()
                .starts_with("last-known-good 在候选阶段失效") =>
        {
            restore_last_known_good(cycle.group, cycle.root, services, cycle.mode)?;
            return Ok(ChangeOutcome::Failed);
        }
        Err(error) => {
            println!("候选准备失败，保留 LKG：{error}");
            return Ok(ChangeOutcome::Failed);
        }
    };
    handle_candidate_result(cycle, watcher, services, result)
}

fn handle_candidate_result(
    cycle: &Cycle<'_>,
    watcher: &SourceWatcher,
    services: &mut Services,
    result: BuildResult,
) -> Result<ChangeOutcome> {
    let candidate = match result {
        BuildResult::Ready(candidate) => *candidate,
        BuildResult::Failed => return Ok(ChangeOutcome::Failed),
        BuildResult::Cancelled => return Ok(ChangeOutcome::Shutdown),
        BuildResult::Superseded => return Ok(ChangeOutcome::Superseded),
        BuildResult::VerifiedNoRestart => {
            if ensure_services_ready(services, cycle.mode).is_err() {
                restore_last_known_good(cycle.group, cycle.root, services, cycle.mode)?;
                return Ok(ChangeOutcome::Failed);
            }
            return Ok(ChangeOutcome::VerifiedNoRestart);
        }
    };
    let probe = probe_candidate(
        cycle.group,
        cycle.root,
        &candidate,
        services,
        cycle.shutdown,
        watcher,
    );
    match candidate_probe_disposition(&probe) {
        CandidateProbeDisposition::Promote => {}
        CandidateProbeDisposition::KeepLastKnownGood => {
            let error = probe.expect_err("失败 disposition 保留原始错误");
            if uncertain(error.as_ref()) {
                return Err(error);
            }
            cleanup_binaries(&candidate)?;
            if ensure_running("last-known-good API", &mut services.api).is_err()
                || ensure_running("last-known-good Worker", &mut services.worker).is_err()
            {
                restore_last_known_good(cycle.group, cycle.root, services, cycle.mode)?;
            }
            return Ok(ChangeOutcome::Failed);
        }
        disposition => {
            cleanup_binaries(&candidate)?;
            return Ok(if disposition == CandidateProbeDisposition::Shutdown {
                ChangeOutcome::Shutdown
            } else {
                ChangeOutcome::Superseded
            });
        }
    }
    if cycle.shutdown.load(Ordering::Acquire) {
        cleanup_binaries(&candidate)?;
        return Ok(ChangeOutcome::Shutdown);
    }
    if !watcher.final_revision_fence(candidate.source_revision) {
        cleanup_binaries(&candidate)?;
        return Ok(ChangeOutcome::Superseded);
    }
    Ok(
        if promote_services(cycle.group, cycle.root, services, candidate, cycle.mode)? {
            ChangeOutcome::Promoted
        } else {
            ChangeOutcome::Failed
        },
    )
}
