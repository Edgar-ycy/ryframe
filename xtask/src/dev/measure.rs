use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    cli::DevOptions,
    process::{ChildGroup, ManagedChild, child_command, stop_child},
    source_edit::SourceEdit,
    watch::{ChangeBatch, SourceWatcher},
    workspace::remove_isolated_directory,
};

use super::{
    build::{DEV_API_FEATURES, cleanup_binaries},
    config::spawn_shutdown_listener,
    health::combine_failures,
    inputs,
    model::{BuildPlan, ChangeOutcome, ServiceLaunchMode},
    orchestrator::{Cycle, build_initial_candidate, process_change, receive_change},
    services::{start_services_in_mode, stop_services},
    snapshot::DevSession,
};

const WATCH_EVENT_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(windows)]
const CANCELLATION_DESCENDANT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const RESULT_FILE_NAME: &str = "cargo-dev-save-result.json";

#[cfg(test)]
#[path = "../../tests/unit/dev/measure.rs"]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SaveCase {
    ConfigOnly,
    ApiOnly,
    WorkerOnly,
    SharedRuntime,
    Locales,
    MigrationOnly,
    ResourceManifest,
    Cancellation,
}

impl SaveCase {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "config-only" => Some(Self::ConfigOnly),
            "api-only" => Some(Self::ApiOnly),
            "worker-only" => Some(Self::WorkerOnly),
            "shared-runtime" => Some(Self::SharedRuntime),
            "locales" => Some(Self::Locales),
            "migration-only" => Some(Self::MigrationOnly),
            "resource-manifest" => Some(Self::ResourceManifest),
            "cancellation" => Some(Self::Cancellation),
            _ => None,
        }
    }

    pub(crate) const fn source(self) -> SaveSource {
        match self {
            Self::ConfigOnly => SaveSource::toml("config/app.dev.toml"),
            Self::ApiOnly => SaveSource::rust("crates/ryframe/src/app.rs"),
            Self::WorkerOnly => SaveSource::rust("crates/ryframe/src/bin/ryframe_worker.rs"),
            Self::SharedRuntime => SaveSource::rust("crates/ryframe-kernel/src/lib.rs"),
            Self::Locales => SaveSource::toml("locales/zh-CN.toml"),
            Self::MigrationOnly => SaveSource::rust("crates/ryframe/src/bin/ryframe_migrate.rs"),
            Self::ResourceManifest => SaveSource::toml("catalog/resources/post.toml"),
            Self::Cancellation => SaveSource::rust("crates/ryframe/src/app.rs"),
        }
    }

    pub(crate) const fn expected_ready(self) -> ReadyKind {
        match self {
            Self::ApiOnly | Self::WorkerOnly | Self::SharedRuntime | Self::Locales => {
                ReadyKind::Promoted
            }
            Self::ConfigOnly | Self::MigrationOnly | Self::ResourceManifest => {
                ReadyKind::VerifiedNoRestart
            }
            Self::Cancellation => ReadyKind::Superseded,
        }
    }

    pub(crate) const fn expected_cargo_invocations(self) -> usize {
        match self {
            Self::ConfigOnly => 0,
            Self::ApiOnly | Self::WorkerOnly | Self::MigrationOnly | Self::ResourceManifest => 1,
            Self::SharedRuntime | Self::Locales => 2,
            Self::Cancellation => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SaveSource {
    pub(crate) relative: &'static str,
    comment_prefix: &'static str,
}

impl SaveSource {
    const fn rust(relative: &'static str) -> Self {
        Self {
            relative,
            comment_prefix: "//",
        }
    }

    const fn toml(relative: &'static str) -> Self {
        Self {
            relative,
            comment_prefix: "#",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReadyKind {
    Promoted,
    VerifiedNoRestart,
    Superseded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct SaveMeasurement {
    pub(crate) schema_version: u8,
    pub(crate) case: SaveCase,
    pub(crate) started_at: String,
    pub(crate) save_to_ready_ms: f64,
    pub(crate) cargo_invocations: usize,
    pub(crate) ready_kind: ReadyKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveMeasurementContract {
    Current,
    LegacyConfigOnlyBaseline,
    LegacyApiWorkerBaseline,
}

pub(crate) fn run(options: &DevOptions) -> Result<()> {
    let root = inputs::workspace(options)?;
    let case = options.case.unwrap_or(SaveCase::SharedRuntime);
    let result_path = options.output.clone();
    let state_root = measurement_state_root(&root, result_path.as_deref());
    remove_stale_result(result_path.as_deref())?;
    let measurement = measure(&root, &state_root, case, options)?;
    if let Some(path) = result_path.as_deref() {
        write_measurement(path, &measurement)?;
    }
    println!("{}", serde_json::to_string(&measurement)?);
    Ok(())
}

fn measure(
    root: &Path,
    state_root: &Path,
    case: SaveCase,
    options: &DevOptions,
) -> Result<SaveMeasurement> {
    if case == SaveCase::Cancellation {
        return measure_cancellation(root, state_root, options);
    }
    let group = ChildGroup::new()?;
    let session = DevSession::prepare_isolated(root, state_root, options)?;
    let mut watcher = SourceWatcher::new_with_inputs(
        root,
        session.capture()?.1,
        crate::watch::SourceRevision::from_value(0),
    )?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let _shutdown_listener = spawn_shutdown_listener(Arc::clone(&shutdown));
    println!("保存反馈测量前置检查：构建并以只读 probe 模式启动独立 LKG。");
    let cycle = Cycle {
        group: &group,
        root,
        session: &session,
        shutdown: &shutdown,
        mode: ServiceLaunchMode::Probe,
    };
    let initial =
        build_initial_candidate(&cycle, &mut watcher)?.ok_or("保存反馈测量前置检查被取消")?;
    let mut services = start_services_in_mode(&group, root, initial, ServiceLaunchMode::Probe)
        .map_err(|error| format!("保存反馈测量前置检查失败：{error}"))?;
    let result = measure_cycle(&cycle, &mut watcher, &mut services, case);
    let binaries = services.binaries.clone();
    let stopped = stop_services(&mut services);
    if stopped.is_err() {
        return combine_failures(result, [("停止保存反馈测量服务，保留现场", stopped)]);
    }
    combine_failures(
        result,
        [("清理保存反馈测量 LKG", cleanup_binaries(&binaries))],
    )
}

fn measure_cancellation(
    root: &Path,
    state_root: &Path,
    options: &DevOptions,
) -> Result<SaveMeasurement> {
    fs::create_dir_all(state_root)?;
    let group = ChildGroup::new()?;
    let session = DevSession::prepare_isolated(root, state_root, options)?;
    let watcher = SourceWatcher::new_with_inputs(
        root,
        session.capture()?.1,
        crate::watch::SourceRevision::from_value(0),
    )?;
    let source_revision = watcher.current_revision();
    let target = state_root.join(format!(
        "cancellation-{}-{}",
        std::process::id(),
        Utc::now().timestamp_millis()
    ));
    fs::create_dir(&target)?;
    #[cfg(windows)]
    let compiler = cancellation_stage(
        cancellation_compiler(root),
        "定位 Rust 编译器或回收 Rustup 进程树",
    )?;
    let build = spawn_cancellation_build(&group, root, &target);
    #[cfg(windows)]
    let build = cancellation_stage(build, "启动 Cargo");
    let mut child = build?;
    #[cfg(windows)]
    cancellation_stage(
        wait_for_compile_descendant(&mut child, &compiler),
        "观察 Cargo 编译后代",
    )?;
    #[cfg(not(windows))]
    wait_for_compile_descendant(&mut child)?;

    let source = SaveCase::Cancellation.source();
    let label = format!(
        "cancel-{}-{}",
        std::process::id(),
        Utc::now().timestamp_millis()
    );
    let saved = SourceEdit::apply(&root.join(source.relative), &label, source.comment_prefix);
    #[cfg(windows)]
    let saved = cancellation_stage(saved, "原子保存测量源码");
    let (mut edit, started) = saved?;
    let outcome = (|| {
        let batch = wait_for_save_event(&watcher, &AtomicBool::new(false), source.relative)?;
        if batch.revision <= source_revision || !watcher.is_superseded(source_revision) {
            return Err("取消测量未观察到新的源码代次".into());
        }
        stop_child(&mut child)?;
        if child
            .active_process_count()?
            .is_some_and(|count| count != 0)
        {
            return Err("取消完成后仍有孤儿进程".into());
        }
        Ok(SaveMeasurement {
            schema_version: 1,
            case: SaveCase::Cancellation,
            started_at: DateTime::<Utc>::from(started.wall_clock).to_rfc3339(),
            save_to_ready_ms: started.monotonic.elapsed().as_secs_f64() * 1_000.0,
            cargo_invocations: 1,
            ready_kind: ReadyKind::Superseded,
        })
    })();
    let outcome = finish_source_edit(outcome, &mut edit);
    if outcome.is_ok() {
        remove_isolated_directory(state_root, &target)?;
    }
    outcome
}

fn spawn_cancellation_build(
    group: &ChildGroup,
    root: &Path,
    target: &Path,
) -> Result<ManagedChild> {
    let mut command = child_command("cargo");
    command
        .args([
            "build",
            "--locked",
            "-p",
            "ryframe",
            "--no-default-features",
            "--features",
            DEV_API_FEATURES,
            "--bin",
            "ryframe",
        ])
        .env("CARGO_TARGET_DIR", target)
        .env("CARGO_INCREMENTAL", "0")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("SCCACHE_RECACHE")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    group.spawn(command)
}

#[cfg(windows)]
fn cancellation_stage<T>(result: Result<T>, stage: &str) -> Result<T> {
    result.map_err(|error| {
        let exit_code = super::failure_exit_code(error.as_ref())
            .or_else(|| crate::process::failure_exit_code(error.as_ref()));
        crate::process::PreservedFailure::new(
            format!("取消测量无法完成阶段（{stage}）：{error}"),
            exit_code,
        )
        .into()
    })
}

#[cfg(windows)]
fn cancellation_compiler(root: &Path) -> Result<same_file::Handle> {
    let mut command = child_command("rustup");
    command
        .args(["which", "rustc"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = crate::process::command_output_result_with_timeout(
        command,
        CANCELLATION_DESCENDANT_TIMEOUT,
    )?;
    if !output.status.success() {
        return Err(format!(
            "取消测量无法解析当前工作树的 Rust 编译器：{}",
            output.status
        )
        .into());
    }
    let line = std::str::from_utf8(&output.stdout)?.trim();
    let compiler = Path::new(line);
    if line.lines().count() != 1 || !compiler.is_absolute() {
        return Err("取消测量的 Rust 编译器路径无效".into());
    }
    Ok(same_file::Handle::from_path(compiler)?)
}

fn wait_for_compile_descendant(
    child: &mut ManagedChild,
    #[cfg(windows)] compiler: &same_file::Handle,
) -> Result<()> {
    let started = Instant::now();
    #[cfg(windows)]
    let deadline = started + CANCELLATION_DESCENDANT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!("取消测量的 Cargo 过早退出：{status}").into());
        }
        #[cfg(windows)]
        let ready = Instant::now() < deadline
            && child
                .active_process_count()?
                .is_some_and(|count| count >= 2)
            && child.has_compiling_rustc(compiler, deadline, rustc_compiles)?;
        #[cfg(windows)]
        if Instant::now() >= deadline {
            return Err("取消测量未观察到 Cargo 编译后代".into());
        }
        #[cfg(not(windows))]
        let ready = started.elapsed() >= Duration::from_millis(250);
        if ready {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(any(windows, test))]
fn rustc_compiles(argv: &[String]) -> bool {
    let Some((_, arguments)) = argv.split_first() else {
        return false;
    };
    let has_value = |flag: &str| {
        arguments
            .windows(2)
            .any(|pair| pair[0] == flag && !pair[1].is_empty() && !pair[1].starts_with('-'))
            || arguments.iter().any(|argument| {
                argument
                    .strip_prefix(flag)
                    .and_then(|suffix| suffix.strip_prefix('='))
                    .is_some_and(|value| !value.is_empty())
            })
    };

    // Cargo 探测目标平台时也可能传入 crate 名，必须确认要求实际生成产物。
    !arguments.iter().any(|argument| {
        matches!(
            argument.as_str(),
            "-vV" | "-Vv" | "-V" | "--version" | "--print"
        ) || argument.starts_with("--print=")
    }) && has_value("--crate-name")
        && has_value("--emit")
}

fn measure_cycle(
    cycle: &Cycle<'_>,
    watcher: &mut SourceWatcher,
    services: &mut super::model::Services,
    case: SaveCase,
) -> Result<SaveMeasurement> {
    let source = case.source();
    let (_, roots, _) = cycle.session.capture()?;
    let path = match case {
        SaveCase::ConfigOnly => roots.config_dir.join("app.dev.toml"),
        SaveCase::Locales => roots.locales_dir.join("zh-CN.toml"),
        _ => cycle.root.join(source.relative),
    };
    let label = format!("{}-{}", std::process::id(), Utc::now().timestamp_millis());
    let (mut edit, started) = SourceEdit::apply(&path, &label, source.comment_prefix)?;
    let started_at = DateTime::<Utc>::from(started.wall_clock);
    let outcome = (|| {
        let batch = wait_for_save_event(watcher, cycle.shutdown, source.relative)?;
        let plan = BuildPlan::from_changes(&batch);
        let expected_cargo_invocations = plan.cargo_invocations();
        if expected_cargo_invocations != case.expected_cargo_invocations() {
            return Err(format!(
                "保存场景 {case:?} 的 Cargo 计划漂移：期望 {}，实际 {expected_cargo_invocations}",
                case.expected_cargo_invocations()
            )
            .into());
        }
        let cargo_counter = AtomicUsize::new(0);
        let outcome = process_change(cycle, watcher, services, Some(&cargo_counter), batch)?;
        let ready_kind = ready_kind(case, outcome)?;
        let cargo_invocations = cargo_counter.load(Ordering::Relaxed);
        if cargo_invocations != expected_cargo_invocations {
            return Err(format!(
                "保存周期 Cargo 调用数漂移：计划 {expected_cargo_invocations}，实际 {cargo_invocations}"
            )
            .into());
        }
        Ok(SaveMeasurement {
            schema_version: 1,
            case,
            started_at: started_at.to_rfc3339(),
            save_to_ready_ms: started.monotonic.elapsed().as_secs_f64() * 1_000.0,
            cargo_invocations,
            ready_kind,
        })
    })();
    finish_source_edit(outcome, &mut edit)
}

fn wait_for_save_event(
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    expected: &str,
) -> Result<ChangeBatch> {
    let deadline = Instant::now() + WATCH_EVENT_TIMEOUT;
    loop {
        if shutdown.load(Ordering::Acquire) {
            return Err("保存反馈测量被取消".into());
        }
        if Instant::now() >= deadline {
            return Err(format!("监听器未在期限内收到保存事件：{expected}").into());
        }
        let Some(batch) = receive_change(watcher)? else {
            continue;
        };
        let expected_paths = BTreeSet::from([expected.to_owned()]);
        if batch.paths != expected_paths {
            return Err(format!(
                "保存反馈测量期间检测到额外源码变更，拒绝污染样本：{:?}",
                batch.paths
            )
            .into());
        }
        return Ok(batch);
    }
}

pub(crate) fn ready_kind(case: SaveCase, outcome: ChangeOutcome) -> Result<ReadyKind> {
    let observed = match outcome {
        ChangeOutcome::Promoted => ReadyKind::Promoted,
        ChangeOutcome::VerifiedNoRestart => ReadyKind::VerifiedNoRestart,
        ChangeOutcome::Failed => return Err("保存周期失败，last-known-good 保持运行".into()),
        ChangeOutcome::Superseded => ReadyKind::Superseded,
        ChangeOutcome::Ignored => return Err("保存事件被状态机忽略".into()),
        ChangeOutcome::Shutdown => return Err("保存反馈测量被取消".into()),
    };
    if observed == case.expected_ready() {
        Ok(observed)
    } else {
        Err(format!(
            "保存场景 {case:?} 的就绪结果漂移：期望 {:?}，实际 {observed:?}",
            case.expected_ready()
        )
        .into())
    }
}

fn finish_source_edit<T>(outcome: Result<T>, edit: &mut SourceEdit) -> Result<T> {
    let restore = edit.restore();
    match (outcome, restore) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(operation_error), Err(restore_error)) => Err(format!(
            "保存反馈测量失败：{operation_error}；源码还原也失败：{restore_error}"
        )
        .into()),
    }
}

fn measurement_state_root(root: &Path, _result_path: Option<&Path>) -> PathBuf {
    // DevEx 的结果目录包含时间戳、套件和成对测量标识。若把 LKG 二进制也放在
    // 该目录内，Windows 在 CreateProcess 时可能因完整可执行文件路径过长而返回
    // ERROR_PATH_NOT_FOUND。结果仍由调用方指定的位置保存，运行态快照则固定放在
    // 较短的本地隔离根中。
    root.join(".local-tests/dev-runtime")
        .join(format!("devex-{}", std::process::id()))
}

fn remove_stale_result(path: Option<&Path>) -> Result<()> {
    if let Some(path) = path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn write_measurement(path: &Path, measurement: &SaveMeasurement) -> Result<()> {
    let mut document = serde_json::to_vec_pretty(measurement)?;
    document.push(b'\n');
    fs::write(path, document)?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn read_measurement(path: &Path) -> Result<SaveMeasurement> {
    read_measurement_with_contract(path, SaveMeasurementContract::Current)
}

pub(crate) fn read_measurement_with_contract(
    path: &Path,
    contract: SaveMeasurementContract,
) -> Result<SaveMeasurement> {
    let measurement: SaveMeasurement = serde_json::from_slice(&fs::read(path)?)?;
    let (expected_cargo_invocations, expected_ready_kind) = match contract {
        SaveMeasurementContract::Current => (
            measurement.case.expected_cargo_invocations(),
            measurement.case.expected_ready(),
        ),
        SaveMeasurementContract::LegacyConfigOnlyBaseline
            if measurement.case == SaveCase::ConfigOnly =>
        {
            (1, ReadyKind::Promoted)
        }
        SaveMeasurementContract::LegacyConfigOnlyBaseline => {
            return Err("legacy-cargo-dev-v1 只允许 config-only 测量结果".into());
        }
        SaveMeasurementContract::LegacyApiWorkerBaseline
            if matches!(measurement.case, SaveCase::ApiOnly | SaveCase::WorkerOnly) =>
        {
            (1, ReadyKind::Promoted)
        }
        SaveMeasurementContract::LegacyApiWorkerBaseline => {
            return Err("legacy-cargo-dev-v2 只允许 api-only 或 worker-only 测量结果".into());
        }
    };
    if measurement.schema_version != 1
        || !measurement.save_to_ready_ms.is_finite()
        || measurement.save_to_ready_ms < 0.0
        || DateTime::parse_from_rfc3339(&measurement.started_at).is_err()
        || measurement.ready_kind != expected_ready_kind
        || measurement.cargo_invocations != expected_cargo_invocations
    {
        return Err("cargo-dev-save 结果 schema、时间或场景不变量无效".into());
    }
    Ok(measurement)
}
