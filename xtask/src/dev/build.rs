use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use crate::{
    Result,
    process::{ChildGroup, ManagedChild, child_command, stop_child},
    watch::SourceWatcher,
};

use super::{
    model::{ArtifactAction, Binaries, BuildPlan, BuildResult, MigrationValidation},
    snapshot::DevSession,
    snapshot::{RuntimeSnapshot, cleanup_runtime},
    tool_self_changed_error,
};

pub(super) use super::snapshot::cleanup_binaries;

#[path = "build/targets.rs"]
mod targets;
pub(crate) use targets::DEV_API_FEATURES;
use targets::{build_targets, requested_targets};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StepResult<T> {
    Complete(T),
    Failed,
    Superseded,
    Cancelled,
}

enum WaitResult {
    Complete(ExitStatus),
    Superseded,
    Cancelled,
}

pub(crate) struct BuildContext<'a> {
    pub(super) group: &'a ChildGroup,
    pub(super) root: &'a Path,
    session: &'a DevSession,
    pub(super) shutdown: &'a AtomicBool,
    pub(super) watcher: &'a SourceWatcher,
    pub(super) cargo_invocations: Option<&'a AtomicUsize>,
    cargo_executable: &'a Path,
}

impl<'a> BuildContext<'a> {
    pub(crate) const fn new(
        group: &'a ChildGroup,
        root: &'a Path,
        session: &'a DevSession,
        shutdown: &'a AtomicBool,
        watcher: &'a SourceWatcher,
        cargo_executable: &'a Path,
    ) -> Self {
        Self {
            group,
            root,
            session,
            shutdown,
            watcher,
            cargo_invocations: None,
            cargo_executable,
        }
    }

    pub(crate) fn with_cargo_counter(mut self, counter: &'a AtomicUsize) -> Self {
        self.cargo_invocations = Some(counter);
        self
    }

    fn cargo_command(&self) -> Command {
        child_command(self.cargo_executable)
    }
}

pub(crate) fn build_candidate(
    context: &BuildContext<'_>,
    plan: &BuildPlan,
    previous: Option<&Binaries>,
    lkg_check: Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<BuildResult> {
    let started = Instant::now();
    let mut lkg_check = lkg_check;
    if plan.tool_self_changed {
        return Err(tool_self_changed_error());
    }
    if let Some(interruption) = build_interruption(context.shutdown, context.watcher, plan) {
        return Ok(interruption);
    }
    let (captured, roots, targets) = context.session.capture()?;
    let runtime = context.session.snapshot_inputs(
        plan.source_revision,
        captured,
        &roots.locales_dir,
        previous,
    )?;
    let artifacts = match prepare_artifacts(context, plan, &mut lkg_check)? {
        StepResult::Complete(artifacts) => artifacts,
        terminal => {
            cleanup_runtime(&runtime)?;
            return Ok(terminal_build_result(terminal));
        }
    };
    if let Some(interruption) = build_interruption(context.shutdown, context.watcher, plan) {
        cleanup_runtime(&runtime)?;
        return Ok(interruption);
    }
    let migration = validate_candidate_migration(
        context,
        plan,
        &artifacts,
        &runtime,
        &targets,
        &mut lkg_check,
    )?;
    if !matches!(migration, StepResult::Complete(())) {
        cleanup_runtime(&runtime)?;
        return Ok(terminal_build_result(migration));
    }
    if context.watcher.is_superseded(plan.source_revision) {
        cleanup_runtime(&runtime)?;
        return Ok(BuildResult::Superseded);
    }
    let candidate = if plan.restart_pair {
        Some(stage_candidate(
            context.session,
            plan,
            previous,
            &artifacts,
            runtime,
        )?)
    } else {
        cleanup_runtime(&runtime)?;
        None
    };
    println!(
        "候选计划完成（r{}，{:.1}s）。",
        plan.source_revision.value(),
        started.elapsed().as_secs_f64()
    );
    Ok(match candidate {
        Some(bundle) => BuildResult::Ready(Box::new(bundle)),
        None => BuildResult::VerifiedNoRestart,
    })
}

fn prepare_artifacts(
    context: &BuildContext<'_>,
    plan: &BuildPlan,
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<BTreeMap<String, PathBuf>>> {
    if plan.resource_check {
        match run_resource_check(context, plan, lkg_check)? {
            StepResult::Complete(()) => {}
            StepResult::Failed => return Ok(StepResult::Failed),
            StepResult::Superseded => return Ok(StepResult::Superseded),
            StepResult::Cancelled => return Ok(StepResult::Cancelled),
        }
    }
    let targets = requested_targets(plan);
    if targets.is_empty() {
        Ok(StepResult::Complete(BTreeMap::new()))
    } else {
        build_targets(context, plan, &targets, lkg_check)
    }
}

fn validate_candidate_migration(
    context: &BuildContext<'_>,
    plan: &BuildPlan,
    artifacts: &BTreeMap<String, PathBuf>,
    runtime: &RuntimeSnapshot,
    targets: &[String],
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<()>> {
    if !plan.migrate {
        return Ok(StepResult::Complete(()));
    }
    let migrate = artifacts
        .get("ryframe-migrate")
        .ok_or("迁移验证计划缺少 ryframe-migrate 构建产物")?;
    run_migration_validation(
        context,
        migrate,
        &runtime.directory.join("migrate"),
        targets,
        plan,
        lkg_check,
    )
}

fn build_interruption(
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    plan: &BuildPlan,
) -> Option<BuildResult> {
    if watcher.is_superseded(plan.source_revision) {
        Some(BuildResult::Superseded)
    } else if shutdown.load(Ordering::Acquire) {
        Some(BuildResult::Cancelled)
    } else {
        None
    }
}

fn terminal_build_result<T>(result: StepResult<T>) -> BuildResult {
    match result {
        StepResult::Failed => BuildResult::Failed,
        StepResult::Superseded => BuildResult::Superseded,
        StepResult::Cancelled => BuildResult::Cancelled,
        StepResult::Complete(_) => unreachable!("完成结果必须由调用方提取载荷"),
    }
}

fn run_resource_check(
    context: &BuildContext<'_>,
    plan: &BuildPlan,
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<()>> {
    let mut command = context.cargo_command();
    command
        .args(["xtask", "generate", "resource", "--all", "--check"])
        .current_dir(context.root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    record_cargo_invocation(context.cargo_invocations);
    let mut child = context.group.spawn(command)?;
    Ok(
        match wait_command(
            &mut child,
            context.shutdown,
            context.watcher,
            plan,
            lkg_check,
        )? {
            WaitResult::Complete(status) if status.success() => StepResult::Complete(()),
            WaitResult::Complete(_) => StepResult::Failed,
            WaitResult::Superseded => StepResult::Superseded,
            WaitResult::Cancelled => StepResult::Cancelled,
        },
    )
}

pub(super) fn record_cargo_invocation(counter: Option<&AtomicUsize>) {
    if let Some(counter) = counter {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn run_migration_validation(
    context: &BuildContext<'_>,
    migrate: &Path,
    config_dir: &Path,
    targets: &[String],
    plan: &BuildPlan,
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<()>> {
    for arguments in migration_arguments(plan.migration, targets)? {
        if let Some(interruption) = build_step_interruption(context.shutdown, context.watcher, plan)
        {
            return Ok(interruption);
        }
        let mut verify = Command::new(migrate);
        verify
            .args(arguments)
            .arg("--config-dir")
            .arg(config_dir)
            .env("APP_ENV", "dev")
            .current_dir(context.root)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let mut child = context.group.spawn(verify)?;
        match wait_command(
            &mut child,
            context.shutdown,
            context.watcher,
            plan,
            lkg_check,
        )? {
            WaitResult::Complete(status) if status.success() => {}
            WaitResult::Complete(_) => return Ok(StepResult::Failed),
            WaitResult::Superseded => return Ok(StepResult::Superseded),
            WaitResult::Cancelled => return Ok(StepResult::Cancelled),
        }
    }
    Ok(StepResult::Complete(()))
}

fn migration_arguments(
    validation: MigrationValidation,
    targets: &[String],
) -> Result<Vec<Vec<String>>> {
    match validation {
        MigrationValidation::None => Ok(Vec::new()),
        MigrationValidation::StandaloneControl => {
            Ok(vec![vec!["control".to_owned(), "verify".to_owned()]])
        }
        MigrationValidation::StandaloneTenant => tenant_migration_arguments(targets),
        MigrationValidation::StandaloneControlAndTenant => {
            let mut arguments = vec![vec!["control".to_owned(), "verify".to_owned()]];
            arguments.extend(tenant_migration_arguments(targets)?);
            Ok(arguments)
        }
    }
}

fn tenant_migration_arguments(targets: &[String]) -> Result<Vec<Vec<String>>> {
    if targets.is_empty() {
        return Err("租户迁移发生变化，但 development.verify_tenant_targets 未配置；请在工具文件登记目标，拒绝跳过验证".into());
    }
    Ok(targets
        .iter()
        .map(|target| {
            vec![
                "tenant-data".into(),
                "verify".into(),
                "--target".into(),
                target.clone(),
            ]
        })
        .collect())
}

fn wait_command(
    child: &mut ManagedChild,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    plan: &BuildPlan,
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<WaitResult> {
    loop {
        if shutdown.load(Ordering::Acquire) {
            stop_child(child)?;
            return Ok(WaitResult::Cancelled);
        }
        if watcher.is_superseded(plan.source_revision) {
            stop_child(child)?;
            return Ok(WaitResult::Superseded);
        }
        if let Some(status) = child.try_wait()? {
            return Ok(WaitResult::Complete(status));
        }
        if let Some(check) = lkg_check.as_mut()
            && let Err(error) = (**check)()
        {
            stop_child(child)?;
            return Err(format!("last-known-good 在候选阶段失效：{error}").into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn build_step_interruption<T>(
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    plan: &BuildPlan,
) -> Option<StepResult<T>> {
    if shutdown.load(Ordering::Acquire) {
        Some(StepResult::Cancelled)
    } else if watcher.is_superseded(plan.source_revision) {
        Some(StepResult::Superseded)
    } else {
        None
    }
}

fn stage_candidate(
    session: &DevSession,
    plan: &BuildPlan,
    previous: Option<&Binaries>,
    artifacts: &BTreeMap<String, PathBuf>,
    runtime: RuntimeSnapshot,
) -> Result<Binaries> {
    let api_source = artifact_or_lkg("ryframe", plan.api, artifacts, previous)?;
    let worker_source = artifact_or_lkg("ryframe-worker", plan.worker, artifacts, previous)?;
    session.install_generation(plan.source_revision, api_source, worker_source, runtime)
}

fn artifact_or_lkg<'a>(
    target: &str,
    action: ArtifactAction,
    artifacts: &'a BTreeMap<String, PathBuf>,
    previous: Option<&'a Binaries>,
) -> Result<&'a Path> {
    match action {
        ArtifactAction::Rebuild => artifacts
            .get(target)
            .map(PathBuf::as_path)
            .ok_or_else(|| format!("缺少 {target} 的新构建产物").into()),
        ArtifactAction::ReuseLkg => {
            let previous = previous.ok_or("计划要求复用 LKG，但当前没有可用版本")?;
            Ok(if target == "ryframe" {
                previous.api.as_path()
            } else {
                previous.worker.as_path()
            })
        }
        ArtifactAction::NotNeeded => Err(format!("运行时候选不能省略 {target}").into()),
    }
}
