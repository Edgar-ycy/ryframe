use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    Result,
    process::{ChildGroup, child_command, stop_child},
    watch::SourceWatcher,
};

use super::model::{ArtifactAction, Binaries, BuildPlan, BuildResult, MigrationValidation};

enum StepResult<T> {
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

pub(super) fn build_candidate(
    group: &ChildGroup,
    root: &Path,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    plan: &BuildPlan,
    previous: Option<&Binaries>,
    lkg_check: Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<BuildResult> {
    let started = Instant::now();
    let mut lkg_check = lkg_check;
    if plan.tool_self_changed {
        return Err("xtask 自身已变化，请重新运行 `cargo dev`".into());
    }

    if plan.resource_check {
        match run_resource_check(group, root, shutdown, watcher, plan, &mut lkg_check)? {
            StepResult::Complete(()) => {}
            StepResult::Failed => return Ok(BuildResult::Failed),
            StepResult::Superseded => return Ok(BuildResult::Superseded),
            StepResult::Cancelled => return Ok(BuildResult::Cancelled),
        }
    }

    let targets = requested_targets(plan);
    let artifacts = if targets.is_empty() {
        BTreeMap::new()
    } else {
        match build_targets(
            group,
            root,
            shutdown,
            watcher,
            plan,
            &targets,
            &mut lkg_check,
        )? {
            StepResult::Complete(artifacts) => artifacts,
            StepResult::Failed => return Ok(BuildResult::Failed),
            StepResult::Superseded => return Ok(BuildResult::Superseded),
            StepResult::Cancelled => return Ok(BuildResult::Cancelled),
        }
    };

    if watcher.is_superseded(plan.source_revision) {
        return Ok(BuildResult::Superseded);
    }
    if shutdown.load(Ordering::Acquire) {
        return Ok(BuildResult::Cancelled);
    }

    let mut candidate = if plan.restart_pair {
        Some(stage_candidate(root, plan, previous, &artifacts)?)
    } else {
        None
    };
    let config_dir = candidate
        .as_ref()
        .map_or_else(|| root.join("config"), |bundle| bundle.config_dir.clone());
    let locales_dir = candidate
        .as_ref()
        .map_or_else(|| root.join("locales"), |bundle| bundle.locales_dir.clone());

    if plan.migrate {
        let Some(migrate) = artifacts.get("ryframe-migrate") else {
            if let Some(bundle) = candidate.take() {
                let _ = cleanup_binaries(root, &bundle);
            }
            return Err("迁移验证计划缺少 ryframe-migrate 构建产物".into());
        };
        match run_migration_validation(
            group,
            root,
            migrate,
            &config_dir,
            &locales_dir,
            shutdown,
            watcher,
            plan,
            &mut lkg_check,
        )? {
            StepResult::Complete(()) => {}
            StepResult::Failed => {
                cleanup_candidate(root, candidate.take());
                return Ok(BuildResult::Failed);
            }
            StepResult::Superseded => {
                cleanup_candidate(root, candidate.take());
                return Ok(BuildResult::Superseded);
            }
            StepResult::Cancelled => {
                cleanup_candidate(root, candidate.take());
                return Ok(BuildResult::Cancelled);
            }
        }
    }

    if watcher.is_superseded(plan.source_revision) {
        cleanup_candidate(root, candidate.take());
        return Ok(BuildResult::Superseded);
    }
    println!(
        "候选计划完成（r{}，{:.1}s）。",
        plan.source_revision.value(),
        started.elapsed().as_secs_f64()
    );
    Ok(match candidate {
        Some(bundle) => BuildResult::Ready(bundle),
        None => BuildResult::VerifiedNoRestart,
    })
}

fn cleanup_candidate(root: &Path, candidate: Option<Binaries>) {
    if let Some(bundle) = candidate {
        let _ = cleanup_binaries(root, &bundle);
    }
}

fn requested_targets(plan: &BuildPlan) -> Vec<&'static str> {
    let mut targets = Vec::new();
    if plan.api == ArtifactAction::Rebuild {
        targets.push("ryframe");
    }
    if plan.worker == ArtifactAction::Rebuild {
        targets.push("ryframe-worker");
    }
    if plan.migrate {
        targets.push("ryframe-migrate");
    }
    targets
}

fn run_resource_check(
    group: &ChildGroup,
    root: &Path,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    plan: &BuildPlan,
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<()>> {
    let mut command = child_command("cargo");
    command
        .args(["resource", "--all", "--check"])
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = group.spawn(&mut command)?;
    Ok(
        match wait_command(&mut child, shutdown, watcher, plan, lkg_check)? {
            WaitResult::Complete(status) if status.success() => StepResult::Complete(()),
            WaitResult::Complete(_) => StepResult::Failed,
            WaitResult::Superseded => StepResult::Superseded,
            WaitResult::Cancelled => StepResult::Cancelled,
        },
    )
}

fn build_targets(
    group: &ChildGroup,
    root: &Path,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    plan: &BuildPlan,
    targets: &[&str],
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<BTreeMap<String, PathBuf>>> {
    let mut build = child_command("cargo");
    build
        .args(["build", "--locked", "-p", "ryframe"])
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    for target in targets {
        build.args(["--bin", target]);
    }
    build.arg("--message-format=json-render-diagnostics");

    let mut child = group.spawn(&mut build)?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Cargo JSON 输出未建立 stdout pipe")?;
    let reader =
        thread::spawn(move || read_cargo_artifacts(stdout).map_err(|error| error.to_string()));
    let wait = wait_command(&mut child, shutdown, watcher, plan, lkg_check)?;
    let artifacts = reader
        .join()
        .map_err(|_| "Cargo JSON 读取线程异常退出")?
        .map_err(|error| format!("无法读取 Cargo JSON 输出：{error}"))?;
    Ok(match wait {
        WaitResult::Complete(status) if status.success() => {
            for target in targets {
                if !artifacts.contains_key(*target) {
                    return Err(format!("Cargo 未报告 {target} 的 executable 产物").into());
                }
            }
            StepResult::Complete(artifacts)
        }
        WaitResult::Complete(_) => StepResult::Failed,
        WaitResult::Superseded => StepResult::Superseded,
        WaitResult::Cancelled => StepResult::Cancelled,
    })
}

fn read_cargo_artifacts(stdout: impl std::io::Read) -> Result<BTreeMap<String, PathBuf>> {
    let mut artifacts = BTreeMap::new();
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            println!("{line}");
            continue;
        };
        match message.get("reason").and_then(Value::as_str) {
            Some("compiler-artifact") => {
                let name = message
                    .pointer("/target/name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let executable = message
                    .get("executable")
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                if let (Some(name), Some(executable)) = (name, executable) {
                    artifacts.insert(name, executable);
                }
            }
            Some("compiler-message") => {
                if let Some(rendered) = message.pointer("/message/rendered").and_then(Value::as_str)
                {
                    eprint!("{rendered}");
                }
            }
            _ => {}
        }
    }
    Ok(artifacts)
}

#[allow(clippy::too_many_arguments)]
fn run_migration_validation(
    group: &ChildGroup,
    root: &Path,
    migrate: &Path,
    config_dir: &Path,
    locales_dir: &Path,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    plan: &BuildPlan,
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<StepResult<()>> {
    let argument_sets = migration_arguments(plan.migration)?;
    for arguments in argument_sets {
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
    child: &mut Child,
    shutdown: &AtomicBool,
    watcher: &SourceWatcher,
    plan: &BuildPlan,
    lkg_check: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<WaitResult> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(WaitResult::Complete(status));
        }
        if shutdown.load(Ordering::Acquire) {
            stop_child(child)?;
            return Ok(WaitResult::Cancelled);
        }
        if watcher.is_superseded(plan.source_revision) {
            stop_child(child)?;
            return Ok(WaitResult::Superseded);
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

fn stage_candidate(
    root: &Path,
    plan: &BuildPlan,
    previous: Option<&Binaries>,
    artifacts: &BTreeMap<String, PathBuf>,
) -> Result<Binaries> {
    let name = format!(
        "g{}-{}-{}",
        plan.source_revision.value(),
        std::process::id(),
        nonce()?
    );
    let generation_parent = root.join("target/xtask/dev");
    let runtime_parent = root.join(".local-tests/dev-runtime");
    fs::create_dir_all(&generation_parent)?;
    fs::create_dir_all(&runtime_parent)?;
    let generation_staged = generation_parent.join(format!(".{name}.copying"));
    let generation_dir = generation_parent.join(&name);
    let runtime_staged = runtime_parent.join(format!(".{name}.copying"));
    let runtime_dir = runtime_parent.join(&name);
    fs::create_dir_all(generation_staged.join("bin"))?;
    fs::create_dir_all(&runtime_staged)?;

    let result = (|| {
        copy_tree(&root.join("config"), &runtime_staged.join("config"))?;
        copy_tree(&root.join("locales"), &runtime_staged.join("locales"))?;
        let api_source = artifact_or_lkg("ryframe", plan.api, artifacts, previous)?;
        let worker_source = artifact_or_lkg("ryframe-worker", plan.worker, artifacts, previous)?;
        let api_name = binary_name("ryframe");
        let worker_name = binary_name("ryframe-worker");
        fs::copy(api_source, generation_staged.join("bin").join(&api_name))?;
        fs::copy(
            worker_source,
            generation_staged.join("bin").join(&worker_name),
        )?;

        let manifest = serde_json::json!({
            "source_revision": plan.source_revision.value(),
            "config_sha256": directory_hash(&runtime_staged.join("config"))?,
            "locales_sha256": directory_hash(&runtime_staged.join("locales"))?,
        });
        fs::write(
            generation_staged.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        fs::rename(&runtime_staged, &runtime_dir)?;
        if let Err(error) = fs::rename(&generation_staged, &generation_dir) {
            let _ = fs::remove_dir_all(&runtime_dir);
            return Err(error.into());
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&generation_staged);
        let _ = fs::remove_dir_all(&runtime_staged);
        return Err(error);
    }

    Ok(Binaries {
        source_revision: plan.source_revision,
        api: generation_dir.join("bin").join(binary_name("ryframe")),
        worker: generation_dir
            .join("bin")
            .join(binary_name("ryframe-worker")),
        config_dir: runtime_dir.join("config"),
        locales_dir: runtime_dir.join("locales"),
        generation_dir,
        runtime_dir,
    })
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

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    if !source.is_dir() {
        return Err(format!("运行输入目录不存在：{}", source.display()).into());
    }
    fs::create_dir_all(target)?;
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&source_path, &target_path)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(source_path, target_path)?;
        }
    }
    Ok(())
}

fn directory_hash(directory: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(directory, directory, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (relative, path) in files {
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(fs::read(path)?);
        digest.update([0]);
    }
    let bytes = digest.finalize();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}")?;
    }
    Ok(output)
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, files)?;
        } else if entry.file_type()?.is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, path));
        }
    }
    Ok(())
}

fn binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

pub(super) fn cleanup_binaries(root: &Path, binaries: &Binaries) -> Result<()> {
    remove_generation_dir(
        &root.join("target/xtask/dev"),
        &binaries.generation_dir,
        "二进制版本",
    )?;
    remove_generation_dir(
        &root.join(".local-tests/dev-runtime"),
        &binaries.runtime_dir,
        "运行输入版本",
    )
}

fn remove_generation_dir(allowed_parent: &Path, generation: &Path, label: &str) -> Result<()> {
    if generation.parent() != Some(allowed_parent) || generation.file_name().is_none() {
        return Err(format!("{label}目录越过清理白名单：{}", generation.display()).into());
    }
    match fs::remove_dir_all(generation) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法清理{label}目录 {}：{error}", generation.display()).into()),
    }
}

fn nonce() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("系统时间早于 Unix epoch：{error}"))?
        .as_nanos())
}
