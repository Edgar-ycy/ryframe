use std::{
    collections::BTreeMap,
    env,
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
    runtime_secrets::RuntimeSecrets,
    snapshot::DevSession,
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
    let artifacts = match prepare_artifacts(context, plan, &mut lkg_check)? {
        StepResult::Complete(artifacts) => artifacts,
        terminal => return Ok(terminal_build_result(terminal)),
    };
    if let Some(interruption) = build_interruption(context.shutdown, context.watcher, plan) {
        return Ok(interruption);
    }
    let mut candidate = if plan.restart_pair {
        Some(stage_candidate(
            context.root,
            context.session,
            plan,
            previous,
            &artifacts,
        )?)
    } else {
        None
    };
    match validate_candidate_migration(
        context.group,
        context.root,
        context.shutdown,
        context.watcher,
        plan,
        &artifacts,
        &mut candidate,
        &mut lkg_check,
    )? {
        StepResult::Complete(()) => {}
        terminal => return Ok(terminal_build_result(terminal)),
    }
    if context.watcher.is_superseded(plan.source_revision) {
        cleanup_candidate(candidate.take());
        return Ok(BuildResult::Superseded);
    }
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

#[allow(clippy::too_many_arguments)]
fn validate_candidate_migration(
    group: &ChildGroup,
    root: &Path,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    plan: &BuildPlan,
    artifacts: &BTreeMap<String, PathBuf>,
    candidate: &mut Option<Binaries>,
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<()>> {
    if !plan.migrate {
        return Ok(StepResult::Complete(()));
    }
    let Some(migrate) = artifacts.get("ryframe-migrate") else {
        cleanup_candidate(candidate.take());
        return Err("迁移验证计划缺少 ryframe-migrate 构建产物".into());
    };
    let config_dir = candidate
        .as_ref()
        .map_or_else(|| root.join("config"), |bundle| bundle.config_dir.clone());
    let locales_dir = candidate
        .as_ref()
        .map_or_else(|| root.join("locales"), |bundle| bundle.locales_dir.clone());
    let captured_secrets;
    let runtime_secrets = if let Some(bundle) = candidate.as_ref() {
        bundle.runtime_secrets.as_ref()
    } else {
        captured_secrets = RuntimeSecrets::capture(&root.join("config"))?;
        &captured_secrets
    };
    let result = run_migration_validation(
        group,
        root,
        migrate,
        &config_dir,
        &locales_dir,
        runtime_secrets,
        shutdown,
        watcher,
        plan,
        lkg_check,
    )?;
    if !matches!(&result, StepResult::Complete(())) {
        cleanup_candidate(candidate.take());
    }
    Ok(result)
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

fn cleanup_candidate(candidate: Option<Binaries>) {
    if let Some(bundle) = candidate {
        let _ = cleanup_binaries(&bundle);
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
    let mut child = context.group.spawn(&mut command)?;
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_migration_validation(
    group: &ChildGroup,
    root: &Path,
    migrate: &Path,
    config_dir: &Path,
    locales_dir: &Path,
    runtime_secrets: &RuntimeSecrets,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    plan: &BuildPlan,
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<()>> {
    let argument_sets = migration_arguments(plan.migration)?;
    for arguments in argument_sets {
        if let Some(interruption) = build_step_interruption(shutdown, watcher, plan) {
            return Ok(interruption);
        }
        let mut verify = Command::new(migrate);
        verify
            .args(arguments)
            .env("APP_ENV", "dev")
            .env("APP_DATABASE_MIGRATION_MODE", "verify")
            .env("APP_CONFIG_DIR", config_dir)
            .env("APP_LOCALES_DIR", locales_dir)
            .current_dir(root)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        runtime_secrets.apply(&mut verify);
        let mut child = group.spawn(&mut verify)?;
        match wait_command(&mut child, shutdown, watcher, plan, lkg_check)? {
            WaitResult::Complete(status) if status.success() => {}
            WaitResult::Complete(_) => return Ok(StepResult::Failed),
            WaitResult::Superseded => return Ok(StepResult::Superseded),
            WaitResult::Cancelled => return Ok(StepResult::Cancelled),
        }
    }
    Ok(StepResult::Complete(()))
}

fn migration_arguments(validation: MigrationValidation) -> Result<Vec<Vec<String>>> {
    match validation {
        MigrationValidation::None => Ok(Vec::new()),
        MigrationValidation::StandaloneControl => {
            Ok(vec![vec!["control".to_owned(), "verify".to_owned()]])
        }
        MigrationValidation::StandaloneTenant => tenant_migration_arguments(),
        MigrationValidation::StandaloneControlAndTenant => {
            let mut arguments = vec![vec!["control".to_owned(), "verify".to_owned()]];
            arguments.extend(tenant_migration_arguments()?);
            Ok(arguments)
        }
    }
}

fn tenant_migration_arguments() -> Result<Vec<Vec<String>>> {
    let targets = env::var("RYFRAME_DEV_VERIFY_TENANT_TARGETS")
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return Err(
            "租户迁移发生变化，但未设置 RYFRAME_DEV_VERIFY_TENANT_TARGETS；拒绝跳过安全验证".into(),
        );
    }
    Ok(targets
        .into_iter()
        .map(|target| {
            vec![
                "tenant-data".to_owned(),
                "verify".to_owned(),
                "--target".to_owned(),
                target,
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
    root: &Path,
    session: &DevSession,
    plan: &BuildPlan,
    previous: Option<&Binaries>,
    artifacts: &BTreeMap<String, PathBuf>,
) -> Result<Binaries> {
    let api_source = artifact_or_lkg("ryframe", plan.api, artifacts, previous)?;
    let worker_source = artifact_or_lkg("ryframe-worker", plan.worker, artifacts, previous)?;
    session.install_generation(
        plan.source_revision,
        api_source,
        worker_source,
        &root.join("config"),
        &root.join("locales"),
    )
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
