use std::{
    collections::BTreeSet,
    env, fs,
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
    process::{ChildGroup, ManagedChild, child_command, stop_child},
    source_edit::SourceEdit,
    watch::{ChangeBatch, SourceWatcher},
    workspace::{remove_isolated_directory, root_dir},
};

use super::{
    build::{DEV_API_FEATURES, cleanup_binaries},
    config::{DevPorts, WorkerIds, spawn_shutdown_listener},
    health::combine_failures,
    model::{BuildPlan, ChangeOutcome, ServiceLaunchMode},
    orchestrator::{build_initial_candidate, process_change, receive_change},
    services::{ensure_initial_ports_available, start_services_in_mode, stop_services},
    snapshot::DevSession,
};

const CASE_ENV: &str = "RYFRAME_DEVEX_SAVE_CASE";
const RESULT_ENV: &str = "RYFRAME_DEVEX_SAVE_RESULT_PATH";
const WATCH_EVENT_TIMEOUT: Duration = Duration::from_secs(10);
const CANCELLATION_DESCENDANT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const RESULT_FILE_NAME: &str = "cargo-dev-save-result.json";

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
    fn from_environment() -> Result<Self> {
        let value = env::var(CASE_ENV).unwrap_or_else(|_| "shared-runtime".to_owned());
        Self::parse(&value).ok_or_else(|| {
            format!(
                "{CASE_ENV} 只允许 config-only、api-only、worker-only、shared-runtime、locales、migration-only、resource-manifest 或 cancellation"
            )
            .into()
        })
    }

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
            Self::ConfigOnly
            | Self::ApiOnly
            | Self::WorkerOnly
            | Self::SharedRuntime
            | Self::Locales => ReadyKind::Promoted,
            Self::MigrationOnly | Self::ResourceManifest => ReadyKind::VerifiedNoRestart,
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
}

pub(crate) fn run() -> Result<()> {
    let root = root_dir();
    let case = SaveCase::from_environment()?;
    let result_path = measurement_result_path();
    let state_root = measurement_state_root(&root, result_path.as_deref());
    remove_stale_result(result_path.as_deref())?;
    let measurement = measure(&root, &state_root, case)?;
    if let Some(path) = result_path.as_deref() {
        write_measurement(path, &measurement)?;
    }
    println!("{}", serde_json::to_string(&measurement)?);
    Ok(())
}

fn measure(root: &Path, state_root: &Path, case: SaveCase) -> Result<SaveMeasurement> {
    if case == SaveCase::Cancellation {
        return measure_cancellation(root, state_root);
    }
    let group = ChildGroup::new()?;
    let watcher = SourceWatcher::new(root)?;
    let session = DevSession::prepare_isolated(state_root, watcher.current_revision())?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let _shutdown_listener = spawn_shutdown_listener(Arc::clone(&shutdown));
    println!("保存反馈测量前置检查：构建并以只读 probe 模式启动独立 LKG。");
    let initial = build_initial_candidate(&group, root, &session, shutdown.as_ref(), &watcher)?
        .ok_or("保存反馈测量前置检查被取消")?;
    let ports = DevPorts::isolated()?;
    ensure_initial_ports_available(ports)?;
    let worker_ids = WorkerIds::isolated(ports.api);
    let mut services = start_services_in_mode(
        &group,
        root,
        initial,
        ports,
        worker_ids,
        ServiceLaunchMode::Probe,
    )
    .map_err(|error| format!("保存反馈测量前置检查失败（配置、依赖或外部服务）：{error}"))?;
    let result = measure_cycle(
        &group,
        root,
        &session,
        &watcher,
        shutdown.as_ref(),
        &mut services,
        ports,
        worker_ids,
        case,
    );
    let binaries = services.binaries.clone();
    combine_failures(
        result,
        [
            ("停止保存反馈测量服务", stop_services(&mut services)),
            ("清理保存反馈测量 LKG", cleanup_binaries(&binaries)),
        ],
    )
}

fn measure_cancellation(root: &Path, state_root: &Path) -> Result<SaveMeasurement> {
    fs::create_dir_all(state_root)?;
    let group = ChildGroup::new()?;
    let watcher = SourceWatcher::new(root)?;
    let source_revision = watcher.current_revision();
    let target = state_root.join(format!(
        "cancellation-{}-{}",
        std::process::id(),
        Utc::now().timestamp_millis()
    ));
    fs::create_dir(&target)?;
    let mut child = spawn_cancellation_build(&group, root, &target)?;
    wait_for_compile_descendant(&mut child, &target)?;

    let source = SaveCase::Cancellation.source();
    let label = format!(
        "cancel-{}-{}",
        std::process::id(),
        Utc::now().timestamp_millis()
    );
    let (mut edit, started) =
        SourceEdit::apply(&root.join(source.relative), &label, source.comment_prefix)?;
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
    group.spawn(&mut command)
}

fn wait_for_compile_descendant(child: &mut ManagedChild, target: &Path) -> Result<()> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!("取消测量的 Cargo 过早退出：{status}").into());
        }
        match child.active_process_count()? {
            Some(count) if count >= 2 && target.join("debug/.fingerprint").is_dir() => {
                return Ok(());
            }
            None if started.elapsed() >= Duration::from_millis(250) => return Ok(()),
            _ if started.elapsed() >= CANCELLATION_DESCENDANT_TIMEOUT => {
                return Err("取消测量未观察到 Cargo 编译后代".into());
            }
            _ => thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn measure_cycle(
    group: &ChildGroup,
    root: &Path,
    session: &DevSession,
    watcher: &SourceWatcher,
    shutdown: &AtomicBool,
    services: &mut super::model::Services,
    ports: DevPorts,
    worker_ids: WorkerIds,
    case: SaveCase,
) -> Result<SaveMeasurement> {
    let source = case.source();
    let path = root.join(source.relative);
    let label = format!("{}-{}", std::process::id(), Utc::now().timestamp_millis());
    let (mut edit, started) = SourceEdit::apply(&path, &label, source.comment_prefix)?;
    let started_at = DateTime::<Utc>::from(started.wall_clock);
    let outcome = (|| {
        let batch = wait_for_save_event(watcher, shutdown, source.relative)?;
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
        let outcome = process_change(
            group,
            root,
            session,
            watcher,
            shutdown,
            services,
            ports,
            worker_ids,
            ServiceLaunchMode::Probe,
            Some(&cargo_counter),
            batch,
        )?;
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

fn measurement_result_path() -> Option<PathBuf> {
    env::var_os(RESULT_ENV).map(PathBuf::from)
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
    let expected_cargo_invocations = match contract {
        SaveMeasurementContract::Current => measurement.case.expected_cargo_invocations(),
        SaveMeasurementContract::LegacyConfigOnlyBaseline
            if measurement.case == SaveCase::ConfigOnly =>
        {
            1
        }
        SaveMeasurementContract::LegacyConfigOnlyBaseline => {
            return Err("legacy-cargo-dev-v1 只允许 config-only 测量结果".into());
        }
    };
    if measurement.schema_version != 1
        || !measurement.save_to_ready_ms.is_finite()
        || measurement.save_to_ready_ms < 0.0
        || DateTime::parse_from_rfc3339(&measurement.started_at).is_err()
        || measurement.ready_kind != measurement.case.expected_ready()
        || measurement.cargo_invocations != expected_cargo_invocations
    {
        return Err("cargo-dev-save 结果 schema、时间或场景不变量无效".into());
    }
    Ok(measurement)
}
