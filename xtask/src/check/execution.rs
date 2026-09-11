use super::{
    cargo_command::{
        backend_package_operation_args, cargo_operation_jobs, ci_test_jobs_from,
        default_test_jobs_from, workspace_clippy_args, workspace_test_args,
    },
    context::{VerifyExecutionContext, ci_target_policy},
    feature::{
        check_feature_registry, feature_matrix_with_jobs, load_feature_registry,
        run_feature_operations, run_feature_tests, validate_feature_registry,
    },
    metrics,
    model::{BackendSnapshotProfile, FrontendProfile},
    plan::{CheckPlanMode, build_task_plan, render_plan, validate_plan},
    policy_tasks::{PYTHON_POLICY_TASKS, PolicyProfile, policy_tasks},
    resource::resource_workspace_compilation,
    selection::{frontend_profile_commands, load_workspace_metadata},
    snapshot::{
        export_and_verify_backend_snapshots, prepare_backend_snapshots,
        prepare_consumer_backend_snapshots, run_consumer_contract,
        stage_committed_backend_snapshots, verify_backend_snapshots,
    },
};
use crate::{
    Result,
    cli::CheckScope,
    process::{
        PreservedFailure, ProcessCancellation, failure_exit_code, is_process_cancellation,
        run as run_process, run_owned, run_owned_with_env, run_pnpm, with_process_cancellation,
        with_process_log,
    },
    workspace::root_dir,
};
use std::{
    collections::BTreeSet,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

mod task_execution;

use task_execution::execute_plan;

pub(crate) const PYTHON_TEST_ARGS: &[&str] = &[
    "-m",
    "unittest",
    "discover",
    "-s",
    "scripts/tests",
    "-p",
    "test_*.py",
];

pub(crate) const PYTHON_ENVIRONMENT_ARGS: &[&str] = &["scripts/check_python_environment.py"];
pub(crate) const SMART_BACKEND_OPERATIONS: &[&str] = &["clippy", "test"];
pub(crate) const SMART_FEATURE_OPERATIONS: &[&str] = &["clippy"];

/// 根据工作树变更执行最小安全检查；完整模式覆盖全部本地门禁。
pub(crate) fn verify(scope: CheckScope, full: bool, frontend_dir: &Path) -> Result<()> {
    let started = Instant::now();
    let mut mode = if full { "完整" } else { "智能" };
    let mut context = VerifyExecutionContext::new(frontend_dir, full)?;
    metrics::begin(
        &context.root,
        &context.frontend_dir,
        scope_label(scope),
        mode,
        context.targets.backend.as_str(),
        context.targets.resource.as_str(),
    );
    let result = (|| {
        let check_plan = build_task_plan(scope, full, &context.root, &context.frontend_dir)?;
        validate_plan(&check_plan)?;
        render_plan(&check_plan);
        if matches!(check_plan.mode, CheckPlanMode::ExpandedFull(_)) {
            mode = "完整（自动扩大）";
            let metrics_root = context.root.clone();
            context.promote_to_full();
            metrics::update_targets(
                &metrics_root,
                context.targets.backend.as_str(),
                context.targets.resource.as_str(),
            );
        }
        execute_plan(&check_plan, &context)
    })();
    let total_seconds = started.elapsed().as_secs_f64();
    println!(
        "cargo xtask check {}：范围={}，模式={mode}，总耗时={:.1}s。",
        if result.is_ok() { "完成" } else { "失败" },
        scope_label(scope),
        total_seconds
    );
    metrics::finish(mode, total_seconds, result.is_ok());
    result
}

const fn scope_label(scope: CheckScope) -> &'static str {
    match scope {
        CheckScope::All => "前后端",
        CheckScope::Backend => "后端",
        CheckScope::Frontend => "前端",
    }
}

pub(crate) fn ci_rust_gate(frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let context = VerifyExecutionContext::new(frontend_dir, true)?;
    let targets = ci_target_policy()?;
    let snapshots = prepare_backend_snapshots(
        &root,
        &[
            BackendSnapshotProfile::OpenApiContract,
            BackendSnapshotProfile::Mysql,
        ]
        .into_iter()
        .collect(),
    )?;
    check_feature_registry(&root)?;
    run_owned(
        &root,
        "cargo",
        &workspace_clippy_args(&targets.backend, context.jobs.backend),
    )?;
    feature_matrix_with_jobs(&root, &targets.backend, context.jobs.backend)?;

    let test_jobs = ci_test_jobs_from(
        std::env::var("RYFRAME_CI_TEST_JOBS").ok().as_deref(),
        cfg!(windows),
        context.jobs.total,
    )?;
    let test_args = workspace_test_args(&targets.backend, test_jobs);
    run_owned_with_env(
        &root,
        "cargo",
        &test_args,
        &snapshots.workspace_test_environment(),
    )?;
    resource_workspace_compilation(
        &root,
        frontend_dir,
        &targets.resource,
        context.jobs.resource,
    )?;
    verify_backend_snapshots(&root, &snapshots)
}

pub(crate) fn ci_consumer_contract(frontend_dir: &Path) -> Result<()> {
    let targets = ci_target_policy()?;
    ci_consumer_contract_with_target(frontend_dir, &targets.backend)
}

pub(crate) fn ci_consumer_contract_with_target(
    frontend_dir: &Path,
    target_dir: &str,
) -> Result<()> {
    require_frontend_dependencies(frontend_dir)?;
    let root = root_dir();
    let profiles = [BackendSnapshotProfile::OpenApiContract]
        .into_iter()
        .collect();
    let snapshots = export_and_verify_backend_snapshots(&root, &profiles, target_dir)?;
    run_consumer_contract(&root, frontend_dir, &snapshots, false)
}

pub(crate) fn ci_consumer_contract_against_committed_snapshot(
    root: &Path,
    frontend_dir: &Path,
) -> Result<()> {
    require_frontend_dependencies(frontend_dir)?;
    let profiles = [BackendSnapshotProfile::OpenApiContract]
        .into_iter()
        .collect();
    let snapshots = prepare_consumer_backend_snapshots(root, &profiles)?;
    stage_committed_backend_snapshots(root, &snapshots)?;
    run_consumer_contract(root, frontend_dir, &snapshots, false)
}

pub(crate) fn run_parallel_tasks<Left, Right>(
    root: &Path,
    left_label: &str,
    left: Left,
    right_label: &str,
    right: Right,
) -> Result<()>
where
    Left: FnOnce() -> Result<()> + Send,
    Right: FnOnce() -> Result<()> + Send,
{
    let logs = root.join("target/verify/logs");
    let cancellation = ProcessCancellation::new();
    let first_failure = Arc::new(Mutex::new(None));
    thread::scope(|scope| {
        let left_log = logs.join(format!("{left_label}.log"));
        let right_log = logs.join(format!("{right_label}.log"));
        let left_cancellation = cancellation.clone();
        let left_failure = Arc::clone(&first_failure);
        let left = scope.spawn(move || {
            run_parallel_branch(
                left_label,
                &left_log,
                &left_cancellation,
                &left_failure,
                left,
            );
        });
        let right_cancellation = cancellation.clone();
        let right_failure = Arc::clone(&first_failure);
        let right = scope.spawn(move || {
            run_parallel_branch(
                right_label,
                &right_log,
                &right_cancellation,
                &right_failure,
                right,
            );
        });
        let _ = left.join();
        let _ = right.join();
    });
    let mut failure = first_failure
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(failure) = failure.take() {
        Err(failure.into())
    } else {
        Ok(())
    }
}

fn run_parallel_branch<Action>(
    label: &str,
    log: &Path,
    cancellation: &ProcessCancellation,
    first_failure: &Mutex<Option<PreservedFailure>>,
    action: Action,
) where
    Action: FnOnce() -> Result<()>,
{
    let result = catch_unwind(AssertUnwindSafe(|| {
        with_process_cancellation(cancellation, || with_process_log(label, log, action))
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) if is_process_cancellation(error.as_ref()) => {}
        Ok(Err(error)) => record_parallel_failure(
            first_failure,
            cancellation,
            PreservedFailure::new(
                format!("并行任务 {label} 失败：{error}"),
                failure_exit_code(error.as_ref()),
            ),
        ),
        Err(payload) => record_parallel_failure(
            first_failure,
            cancellation,
            PreservedFailure::new(
                format!("并行任务 {label} 发生 panic：{}", panic_message(&payload)),
                None,
            ),
        ),
    }
}

fn record_parallel_failure(
    first_failure: &Mutex<Option<PreservedFailure>>,
    cancellation: &ProcessCancellation,
    failure: PreservedFailure,
) {
    let mut slot = first_failure
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.is_none() {
        *slot = Some(failure);
        cancellation.request();
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("未知 panic")
}

fn require_frontend_dependencies(frontend_dir: &Path) -> Result<()> {
    if !frontend_dir.join("node_modules").is_dir() {
        return Err(format!(
            "完整资源切片编译需要前端依赖目录 {}；请先运行 `corepack pnpm install --frozen-lockfile`",
            frontend_dir.join("node_modules").display()
        )
        .into());
    }
    Ok(())
}

fn backend(root: &Path, backend_target: &str, jobs: usize, frontend_dir: &Path) -> Result<()> {
    check_feature_registry(root)?;
    run_process(root, "cargo", &["fmt", "--all", "--", "--check"])?;
    // Clippy 会先完成 Workspace 全目标类型检查，无需再执行覆盖范围更小的 cargo check。
    let clippy = workspace_clippy_args(backend_target, jobs);
    run_owned(root, "cargo", &clippy)?;
    for task in policy_tasks(PolicyProfile::Full) {
        run_owned(root, "python", &task.arguments(root, frontend_dir)?)?;
    }
    Ok(())
}

fn backend_packages(
    context: &VerifyExecutionContext,
    packages: &BTreeSet<String>,
    snapshots: Option<&super::snapshot::BackendSnapshots>,
) -> Result<()> {
    let root = &context.root;
    let metadata = load_workspace_metadata(root)?;
    let registry = load_feature_registry(root)?;
    validate_feature_registry(&metadata, &registry)?;
    println!("Cargo feature 注册表检查通过。");
    run_process(root, "cargo", &["fmt", "--all", "--", "--check"])?;
    let compile_jobs = context.jobs.total;
    for &operation in SMART_BACKEND_OPERATIONS {
        let jobs = cargo_operation_jobs(operation, cfg!(windows), compile_jobs);
        let args = backend_package_operation_args(
            operation,
            packages,
            context.targets.backend.as_str(),
            jobs,
        );
        if operation == "test" {
            let environment = snapshots
                .map(super::snapshot::BackendSnapshots::workspace_test_environment)
                .unwrap_or_default();
            run_owned_with_env(root, "cargo", &args, &environment)?;
        } else {
            run_owned(root, "cargo", &args)?;
        }
    }
    for entry in registry
        .iter()
        .filter(|entry| packages.contains(&entry.package))
    {
        run_feature_operations(
            root,
            &entry.package,
            "最大（智能检查）",
            &entry.maximal,
            SMART_FEATURE_OPERATIONS,
            context.targets.backend.as_str(),
            compile_jobs,
        )?;
        run_feature_tests(
            root,
            entry,
            context.targets.backend.as_str(),
            default_test_jobs_from(cfg!(windows), compile_jobs),
        )?;
    }
    for task in policy_tasks(PolicyProfile::Smart) {
        run_owned(
            root,
            "python",
            &task.arguments(root, &context.frontend_dir)?,
        )?;
    }
    Ok(())
}

pub(super) fn run_removed_identity(root: &Path, frontend_dir: &Path) -> Result<()> {
    let task = PYTHON_POLICY_TASKS
        .iter()
        .find(|task| task.id == "removed-identity")
        .ok_or("服务身份删除检查任务未登记")?;
    run_owned(root, "python", &task.arguments(root, frontend_dir)?)
}

fn frontend_profiles(
    frontend_dir: &Path,
    profiles: &BTreeSet<FrontendProfile>,
    consumer_contract_ran: bool,
) -> Result<()> {
    for command in frontend_profile_commands(profiles, consumer_contract_ran) {
        run_pnpm(frontend_dir, command)?;
    }
    Ok(())
}
