use super::{
    change_surface::{
        analyze_change_surface, append_changed_file_size_warnings, enforce_change_surface,
        load_change_surface_policy, print_change_surface,
    },
    context::{
        BACKEND_CI_TARGET_DIR, BACKEND_SMART_TARGET_DIR, BACKEND_VERIFY_TARGET_DIR,
        RESOURCE_CI_TARGET_DIR, VerifyExecutionContext,
    },
    feature::{
        check_feature_registry, feature_matrix_with_jobs, load_feature_registry,
        run_feature_operations, run_feature_tests, validate_feature_registry,
    },
    metrics,
    model::{BackendSnapshotProfile, FrontendProfile, WorkspaceGraph},
    resource::resource_workspace_compilation,
    selection::{
        changed_paths, classify_changes, complete_verify_selection, frontend_profile_commands,
        load_workspace_graph, load_workspace_metadata, needs_consumer_contract, print_selection,
    },
    snapshot::{
        export_and_verify_backend_snapshots, package_tests_generate_snapshots,
        prepare_backend_snapshots, run_consumer_contract, verify_backend_snapshots,
    },
};
use crate::{
    Result,
    cli::CheckScope,
    process::{run as run_process, run_owned, run_owned_with_env, run_pnpm, with_process_log},
    workspace::root_dir,
};
use std::{collections::BTreeSet, path::Path, thread, time::Instant};

pub(crate) const PYTHON_TEST_ARGS: &[&str] = &[
    "-m",
    "unittest",
    "discover",
    "-s",
    "scripts/tests",
    "-p",
    "test_*.py",
];
pub(crate) const BACKEND_POLICY_SCRIPTS: &[&str] = &[
    "scripts/check_architecture.py",
    "scripts/check_deployment_assets.py",
    "scripts/check_migration_history.py",
    "scripts/check_prerelease_dependencies.py",
    "scripts/check_permission_routes.py",
    "scripts/check_supply_chain.py",
];
pub(crate) const WORKSPACE_CLIPPY_ARGS: &[&str] = &[
    "clippy",
    "--locked",
    "--target-dir",
    BACKEND_VERIFY_TARGET_DIR,
    "--workspace",
    "--all-targets",
    "--all-features",
    "--",
    "-D",
    "warnings",
    "-D",
    "clippy::redundant_clone",
];
pub(crate) const FRONTEND_FULL_NON_CONSUMER_COMMANDS: &[&str] = &[
    "check:workflows",
    "check:dependencies",
    "test:policies",
    "check:source-size",
    "lint",
    "lint:styles",
    "build",
    "check:bundle",
];
pub(crate) const FRONTEND_ONLY_CONTRACT_COMMANDS: &[&str] =
    &["api:check", "typecheck", "test:unit"];
pub(crate) const CONSUMER_OWNED_COMMANDS: &[&str] = &[
    "check:contract",
    "check:api-artifacts",
    "check:api-operations",
    "typecheck",
    "test:unit",
];
pub(crate) const SMART_BACKEND_OPERATIONS: &[&str] = &["clippy", "test"];
pub(crate) const SMART_FEATURE_OPERATIONS: &[&str] = &["clippy"];

pub(crate) fn run(scope: CheckScope, frontend_dir: &Path) -> Result<()> {
    match scope {
        CheckScope::Backend => backend(&root_dir(), BACKEND_SMART_TARGET_DIR),
        CheckScope::Frontend => frontend(frontend_dir),
        CheckScope::All => {
            backend(&root_dir(), BACKEND_SMART_TARGET_DIR)?;
            frontend(frontend_dir)
        }
    }
}

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
        let root = &context.root;
        let includes_backend = matches!(scope, CheckScope::All | CheckScope::Backend);
        let includes_frontend = matches!(scope, CheckScope::All | CheckScope::Frontend);
        let all_backend_changes = changed_paths(root)?;
        let all_frontend_changes = changed_paths(&context.frontend_dir)?;
        let policy = load_change_surface_policy(root)?;
        let mut change_surface =
            analyze_change_surface(&all_backend_changes, &all_frontend_changes, &policy);
        append_changed_file_size_warnings(
            root,
            &context.frontend_dir,
            &all_backend_changes,
            &all_frontend_changes,
            &policy,
            &mut change_surface,
        )?;
        print_change_surface(&change_surface);
        enforce_change_surface(&change_surface)?;

        if full {
            println!("cargo verify 选择完整门禁：显式传入 --full。");
            return full_verify(&context, scope);
        }

        let graph = if includes_backend {
            load_workspace_graph(root)?
        } else {
            WorkspaceGraph::default()
        };
        let backend_changes = if includes_backend {
            all_backend_changes.as_slice()
        } else {
            &[]
        };
        let frontend_changes = if includes_frontend {
            all_frontend_changes.as_slice()
        } else {
            &[]
        };
        let mut selection = classify_changes(backend_changes, frontend_changes, &graph);
        if let Some(reason) = &selection.full_reason {
            mode = "完整（自动扩大）";
            println!("cargo verify 扩大为完整门禁：{reason}");
            let metrics_root = context.root.clone();
            context.promote_to_full();
            metrics::update_targets(
                &metrics_root,
                context.targets.backend.as_str(),
                context.targets.resource.as_str(),
            );
            return full_verify(&context, scope);
        }

        complete_verify_selection(&mut selection, &graph);
        print_selection(&selection);
        let package_tests_generate_snapshots = !selection.backend_packages.is_empty()
            && !selection.backend_snapshot_profiles.is_empty()
            && package_tests_generate_snapshots(
                &selection.backend_snapshot_profiles,
                &selection.backend_packages,
            );
        let mut snapshots = if package_tests_generate_snapshots {
            Some(prepare_backend_snapshots(
                root,
                &selection.backend_snapshot_profiles,
            )?)
        } else {
            None
        };
        if !selection.backend_packages.is_empty() {
            backend_packages(&context, &selection.backend_packages, snapshots.as_ref())?;
        }
        let mut consumer_contract_ran = false;
        if !selection.backend_snapshot_profiles.is_empty() {
            if needs_consumer_contract(&selection.backend_snapshot_profiles) {
                require_frontend_dependencies(&context.frontend_dir)?;
            }
            if let Some(generated) = snapshots.as_ref() {
                verify_backend_snapshots(root, generated)?;
            } else {
                // 纯快照变更没有可复用的 package test，保留聚焦导出路径。
                snapshots = Some(export_and_verify_backend_snapshots(
                    root,
                    &selection.backend_snapshot_profiles,
                    context.targets.backend.as_str(),
                )?);
            }
            if needs_consumer_contract(&selection.backend_snapshot_profiles) {
                run_consumer_contract(
                    root,
                    &context.frontend_dir,
                    snapshots.as_ref().ok_or("后端快照尚未生成")?,
                )?;
                consumer_contract_ran = true;
            }
        }
        if !selection.frontend_profiles.is_empty() {
            frontend_profiles(
                &context.frontend_dir,
                &selection.frontend_profiles,
                consumer_contract_ran,
            )?;
        }
        if selection.backend_packages.is_empty()
            && selection.backend_snapshot_profiles.is_empty()
            && selection.frontend_profiles.is_empty()
        {
            println!("没有需要执行的代码检查；当前变更仅包含文档，或工作树没有变更。");
        }
        Ok(())
    })();
    let total_seconds = started.elapsed().as_secs_f64();
    println!(
        "cargo verify {}：范围={}，模式={mode}，总耗时={:.1}s。",
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

fn full_verify(context: &VerifyExecutionContext, scope: CheckScope) -> Result<()> {
    let root = &context.root;
    let frontend_dir = &context.frontend_dir;
    if matches!(scope, CheckScope::All | CheckScope::Backend) {
        require_frontend_dependencies(frontend_dir)?;
    }
    let backend_enabled = matches!(scope, CheckScope::All | CheckScope::Backend);
    let frontend_enabled = matches!(scope, CheckScope::All | CheckScope::Frontend);
    if backend_enabled && frontend_enabled {
        run_parallel_tasks(
            root,
            "backend-foundation",
            || backend(root, context.targets.backend.as_str()),
            "frontend-foundation",
            || frontend_full_non_consumer(frontend_dir),
        )?;
    } else if backend_enabled {
        backend(root, context.targets.backend.as_str())?;
    } else if frontend_enabled {
        frontend_full_non_consumer(frontend_dir)?;
    }
    let backend_snapshots = if backend_enabled {
        let snapshots = prepare_backend_snapshots(
            root,
            &[
                BackendSnapshotProfile::OpenApiContract,
                BackendSnapshotProfile::Mysql,
            ]
            .into_iter()
            .collect(),
        )?;
        run_process(root, "python", PYTHON_TEST_ARGS)?;
        run_process(
            root,
            "python",
            &["scripts/check_migration_history.py", "--require-frozen"],
        )?;
        let budget = context.jobs;
        println!(
            "完整门禁编译并发：总计={}，主 Workspace={}，资源 Workspace={}",
            budget.total, budget.backend, budget.resource
        );
        run_parallel_tasks(
            root,
            "backend-workspace",
            || {
                feature_matrix_with_jobs(root, context.targets.backend.as_str(), budget.backend)?;
                let args = workspace_test_args(context.targets.backend.as_str(), budget.backend);
                let environment = snapshots.workspace_test_environment();
                run_owned_with_env(root, "cargo", &args, &environment)
            },
            "resource-workspace",
            || {
                resource_workspace_compilation(
                    root,
                    context.targets.resource.as_str(),
                    budget.resource,
                )
            },
        )?;
        verify_backend_snapshots(root, &snapshots)?;
        Some(snapshots)
    } else {
        None
    };
    if matches!(scope, CheckScope::Backend) {
        run_consumer_contract(
            root,
            frontend_dir,
            backend_snapshots
                .as_ref()
                .ok_or("完整后端门禁缺少 OpenAPI 快照")?,
        )?;
    }
    if matches!(scope, CheckScope::All | CheckScope::Frontend) {
        if let Some(snapshots) = backend_snapshots.as_ref() {
            run_consumer_contract(root, frontend_dir, snapshots)?;
        } else {
            // 前端单侧没有可信的后端工作树候选；由前端状态机校验正式或候选契约。
            for command in FRONTEND_ONLY_CONTRACT_COMMANDS {
                run_pnpm(frontend_dir, &[*command])?;
            }
        }
        run_pnpm(frontend_dir, &["test:browser-smoke"])?;
    }
    Ok(())
}

pub(crate) fn ci_rust_gate(frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let context = VerifyExecutionContext::new(frontend_dir, true)?;
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
        &workspace_clippy_args(BACKEND_CI_TARGET_DIR),
    )?;
    feature_matrix_with_jobs(&root, BACKEND_CI_TARGET_DIR, context.jobs.backend)?;

    let test_jobs = ci_test_jobs_from(
        std::env::var("RYFRAME_CI_TEST_JOBS").ok().as_deref(),
        cfg!(windows),
        context.jobs.total,
    )?;
    let test_args = workspace_test_args(BACKEND_CI_TARGET_DIR, test_jobs);
    run_owned_with_env(
        &root,
        "cargo",
        &test_args,
        &snapshots.workspace_test_environment(),
    )?;
    resource_workspace_compilation(&root, RESOURCE_CI_TARGET_DIR, context.jobs.resource)?;
    verify_backend_snapshots(&root, &snapshots)
}

pub(crate) fn ci_consumer_contract(frontend_dir: &Path) -> Result<()> {
    require_frontend_dependencies(frontend_dir)?;
    let root = root_dir();
    let profiles = [BackendSnapshotProfile::OpenApiContract]
        .into_iter()
        .collect();
    let snapshots = export_and_verify_backend_snapshots(&root, &profiles, BACKEND_CI_TARGET_DIR)?;
    run_consumer_contract(&root, frontend_dir, &snapshots)
}

pub(crate) fn ci_test_jobs_from(
    configured: Option<&str>,
    windows: bool,
    fallback: usize,
) -> Result<usize> {
    match configured {
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|jobs| (1..=64).contains(jobs))
            .ok_or_else(|| "RYFRAME_CI_TEST_JOBS 必须是 1 到 64 的整数".into()),
        None if windows => Ok(4),
        None => Ok(fallback.max(1)),
    }
}

fn frontend_full_non_consumer(frontend_dir: &Path) -> Result<()> {
    // consumer:check 负责契约、派生物、operation、类型和单测；这里仅执行互补门禁，
    // 避免完整检查重复运行 vue-tsc 与 Vitest。
    if FRONTEND_FULL_NON_CONSUMER_COMMANDS
        .iter()
        .any(|command| CONSUMER_OWNED_COMMANDS.contains(command))
    {
        return Err("完整前端门禁配置重复执行了 consumer:check 已负责的检查".into());
    }
    for command in FRONTEND_FULL_NON_CONSUMER_COMMANDS {
        run_pnpm(frontend_dir, &[*command])?;
    }
    Ok(())
}

pub(crate) fn workspace_test_args(target_dir: &str, jobs: usize) -> Vec<String> {
    [
        "test",
        "--locked",
        "--target-dir",
        target_dir,
        "--workspace",
        "--all-features",
        "--jobs",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([jobs.to_string()])
    .collect()
}

fn run_parallel_tasks<Left, Right>(
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
    let (left_result, right_result) = thread::scope(|scope| {
        let left_log = logs.join(format!("{left_label}.log"));
        let right_log = logs.join(format!("{right_label}.log"));
        let left = scope.spawn(move || {
            with_process_log(left_label, &left_log, left).map_err(|error| error.to_string())
        });
        let right = scope.spawn(move || {
            with_process_log(right_label, &right_log, right).map_err(|error| error.to_string())
        });
        (left.join(), right.join())
    });
    let left_result = left_result.map_err(|_| format!("并行任务 {left_label} 发生 panic"))?;
    let right_result = right_result.map_err(|_| format!("并行任务 {right_label} 发生 panic"))?;
    match (left_result, right_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(left), Ok(())) => Err(left.into()),
        (Ok(()), Err(right)) => Err(right.into()),
        (Err(left), Err(right)) => {
            Err(format!("并行任务同时失败：{left_label}: {left}；{right_label}: {right}").into())
        }
    }
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

fn backend(root: &Path, backend_target: &str) -> Result<()> {
    check_feature_registry(root)?;
    run_process(root, "cargo", &["fmt", "--all", "--", "--check"])?;
    // Clippy 会先完成 Workspace 全目标类型检查，无需再执行覆盖范围更小的 cargo check。
    let clippy = workspace_clippy_args(backend_target);
    run_owned(root, "cargo", &clippy)?;
    for script in BACKEND_POLICY_SCRIPTS {
        run_process(root, "python", &[script])?;
    }
    Ok(())
}

fn frontend(frontend_dir: &Path) -> Result<()> {
    run_pnpm(frontend_dir, &["check"])
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
    let jobs = context.jobs.total;
    for &operation in SMART_BACKEND_OPERATIONS {
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
            jobs,
        )?;
        run_feature_tests(root, entry, context.targets.backend.as_str(), jobs)?;
    }
    for script in [
        "scripts/check_architecture.py",
        "scripts/check_migration_history.py",
        "scripts/check_permission_routes.py",
    ] {
        run_process(root, "python", &[script])?;
    }
    Ok(())
}

pub(crate) fn backend_package_operation_args(
    operation: &str,
    packages: &BTreeSet<String>,
    target_dir: &str,
    jobs: usize,
) -> Vec<String> {
    let mut args = vec![
        operation.to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        target_dir.to_owned(),
    ];
    for package in packages {
        args.push("-p".to_owned());
        args.push(package.clone());
    }
    if operation == "clippy" {
        args.push("--all-targets".to_owned());
    }
    args.extend(["--jobs".to_owned(), jobs.to_string()]);
    if operation == "clippy" {
        args.extend([
            "--".to_owned(),
            "-D".to_owned(),
            "warnings".to_owned(),
            "-D".to_owned(),
            "clippy::redundant_clone".to_owned(),
        ]);
    }
    args
}

pub(crate) fn workspace_clippy_args(target_dir: &str) -> Vec<String> {
    WORKSPACE_CLIPPY_ARGS
        .iter()
        .map(|argument| {
            if *argument == BACKEND_VERIFY_TARGET_DIR {
                target_dir.to_owned()
            } else {
                (*argument).to_owned()
            }
        })
        .collect()
}

fn frontend_profiles(
    frontend_dir: &Path,
    profiles: &BTreeSet<FrontendProfile>,
    consumer_contract_ran: bool,
) -> Result<()> {
    for command in frontend_profile_commands(profiles, consumer_contract_ran) {
        run_pnpm(frontend_dir, &[command])?;
    }
    Ok(())
}
