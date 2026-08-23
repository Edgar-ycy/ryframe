use std::{collections::BTreeSet, path::Path, time::Instant};

use crate::{
    Result,
    cli::CheckScope,
    process::{run as run_process, run_pnpm},
    workspace::root_dir,
};

use super::{
    feature::{
        check_feature_registry, feature_matrix, load_feature_registry, run_feature_operations,
        run_feature_tests, validate_feature_registry,
    },
    model::{BackendSnapshotProfile, FrontendProfile, WorkspaceGraph},
    selection::{
        changed_paths, classify_changes, complete_verify_selection, frontend_profile_commands,
        load_workspace_graph, load_workspace_metadata, needs_consumer_contract, print_selection,
    },
    snapshot::{export_and_verify_backend_snapshots, run_consumer_contract},
};

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
    "--",
    "-D",
    "warnings",
    "-D",
    "clippy::redundant_clone",
];
/// 完整后端门禁共享的 Cargo 产物目录；feature 指纹由 Cargo 自身隔离。
pub(crate) const BACKEND_VERIFY_TARGET_DIR: &str = "target/verify/backend";
/// 临时资源工作区独立使用的 Cargo 产物目录，避免污染常规后端门禁缓存。
pub(crate) const RESOURCE_VERIFY_TARGET_DIR: &str = "target/verify/resource";
pub(crate) const WORKSPACE_TEST_ARGS: &[&str] = &[
    "test",
    "--locked",
    "--target-dir",
    BACKEND_VERIFY_TARGET_DIR,
    "--workspace",
    "--jobs",
    "2",
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

pub(crate) fn run(scope: CheckScope, frontend_dir: &Path) -> Result<()> {
    match scope {
        CheckScope::Backend => backend(),
        CheckScope::Frontend => frontend(frontend_dir),
        CheckScope::All => {
            backend()?;
            frontend(frontend_dir)
        }
    }
}

/// 根据工作树变更执行最小安全检查；完整模式覆盖全部本地门禁。
pub(crate) fn verify(scope: CheckScope, full: bool, frontend_dir: &Path) -> Result<()> {
    let started = Instant::now();
    let mut mode = if full { "完整" } else { "智能" };
    let result = (|| {
        if full {
            println!("cargo verify 选择完整门禁：显式传入 --full。");
            return full_verify(scope, frontend_dir);
        }

        let root = root_dir();
        let includes_backend = matches!(scope, CheckScope::All | CheckScope::Backend);
        let graph = if includes_backend {
            load_workspace_graph(&root)?
        } else {
            WorkspaceGraph::default()
        };
        let backend_changes = if includes_backend {
            changed_paths(&root)?
        } else {
            Vec::new()
        };
        let frontend_changes = if matches!(scope, CheckScope::All | CheckScope::Frontend) {
            changed_paths(frontend_dir)?
        } else {
            Vec::new()
        };
        let mut selection = classify_changes(&backend_changes, &frontend_changes, &graph);
        if let Some(reason) = &selection.full_reason {
            mode = "完整（自动扩大）";
            println!("cargo verify 扩大为完整门禁：{reason}");
            return full_verify(scope, frontend_dir);
        }

        complete_verify_selection(&mut selection, &graph);
        print_selection(&selection);
        if !selection.backend_packages.is_empty() {
            backend_packages(&selection.backend_packages)?;
        }
        let mut consumer_contract_ran = false;
        if !selection.backend_snapshot_profiles.is_empty() {
            if needs_consumer_contract(&selection.backend_snapshot_profiles) {
                require_frontend_dependencies(frontend_dir)?;
            }
            let snapshots =
                export_and_verify_backend_snapshots(&selection.backend_snapshot_profiles)?;
            if needs_consumer_contract(&selection.backend_snapshot_profiles) {
                run_consumer_contract(frontend_dir, &snapshots)?;
                consumer_contract_ran = true;
            }
        }
        if !selection.frontend_profiles.is_empty() {
            frontend_profiles(
                frontend_dir,
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
    println!(
        "cargo verify {}：范围={}，模式={mode}，总耗时={:.1}s。",
        if result.is_ok() { "完成" } else { "失败" },
        scope_label(scope),
        started.elapsed().as_secs_f64()
    );
    result
}

const fn scope_label(scope: CheckScope) -> &'static str {
    match scope {
        CheckScope::All => "前后端",
        CheckScope::Backend => "后端",
        CheckScope::Frontend => "前端",
    }
}

fn full_verify(scope: CheckScope, frontend_dir: &Path) -> Result<()> {
    if matches!(scope, CheckScope::All | CheckScope::Backend) {
        require_frontend_dependencies(frontend_dir)?;
    }
    if matches!(scope, CheckScope::All | CheckScope::Backend) {
        backend()?;
    }
    if matches!(scope, CheckScope::All | CheckScope::Frontend) {
        frontend_full_non_consumer(frontend_dir)?;
    }
    let backend_snapshots = if matches!(scope, CheckScope::All | CheckScope::Backend) {
        Some(export_and_verify_backend_snapshots(
            &[
                BackendSnapshotProfile::OpenApiContract,
                BackendSnapshotProfile::Mysql,
            ]
            .into_iter()
            .collect(),
        )?)
    } else {
        None
    };
    if matches!(scope, CheckScope::All | CheckScope::Backend) {
        run_process(&root_dir(), "python", PYTHON_TEST_ARGS)?;
        run_process(
            &root_dir(),
            "python",
            &["scripts/check_migration_history.py", "--require-frozen"],
        )?;
        feature_matrix()?;
        run_process(&root_dir(), "cargo", WORKSPACE_TEST_ARGS)?;
        resource_workspace_compilation(frontend_dir)?;
    }
    if matches!(scope, CheckScope::Backend) {
        run_consumer_contract(
            frontend_dir,
            backend_snapshots
                .as_ref()
                .ok_or("完整后端门禁缺少 OpenAPI 快照")?,
        )?;
    }
    if matches!(scope, CheckScope::All | CheckScope::Frontend) {
        if let Some(snapshots) = backend_snapshots.as_ref() {
            run_consumer_contract(frontend_dir, snapshots)?;
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

fn resource_workspace_compilation(frontend_dir: &Path) -> Result<()> {
    require_frontend_dependencies(frontend_dir)?;
    run_process(
        &root_dir(),
        "cargo",
        &[
            "test",
            "--locked",
            "--target-dir",
            RESOURCE_VERIFY_TARGET_DIR,
            "-p",
            "ryframe-generator",
            "--test",
            "resource_workspace_compilation",
            "--",
            "--ignored",
            "--nocapture",
        ],
    )
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

fn backend() -> Result<()> {
    let root = root_dir();
    check_feature_registry(&root)?;
    run_process(&root, "cargo", &["fmt", "--all", "--", "--check"])?;
    // Clippy 会先完成 Workspace 全目标类型检查，无需再执行覆盖范围更小的 cargo check。
    run_process(&root, "cargo", WORKSPACE_CLIPPY_ARGS)?;
    for script in BACKEND_POLICY_SCRIPTS {
        run_process(&root, "python", &[script])?;
    }
    Ok(())
}

fn frontend(frontend_dir: &Path) -> Result<()> {
    run_pnpm(frontend_dir, &["check"])
}

fn backend_packages(packages: &BTreeSet<String>) -> Result<()> {
    let root = root_dir();
    let metadata = load_workspace_metadata(&root)?;
    let registry = load_feature_registry(&root)?;
    validate_feature_registry(&metadata, &registry)?;
    println!("Cargo feature 注册表检查通过。");
    run_process(&root, "cargo", &["fmt", "--all", "--", "--check"])?;
    for operation in ["check", "clippy", "test"] {
        let mut args = vec![operation.to_owned(), "--locked".to_owned()];
        for package in packages {
            args.push("-p".to_owned());
            args.push(package.clone());
        }
        if operation == "check" || operation == "clippy" {
            args.push("--all-targets".to_owned());
        }
        if operation == "clippy" {
            args.extend([
                "--".to_owned(),
                "-D".to_owned(),
                "warnings".to_owned(),
                "-D".to_owned(),
                "clippy::redundant_clone".to_owned(),
            ]);
        }
        run_owned(&root, "cargo", &args)?;
    }
    for entry in registry
        .iter()
        .filter(|entry| packages.contains(&entry.package))
    {
        run_feature_operations(
            &root,
            &entry.package,
            "最大（智能检查）",
            &entry.maximal,
            &["check", "clippy"],
        )?;
        run_feature_tests(&root, entry)?;
    }
    for script in [
        "scripts/check_architecture.py",
        "scripts/check_migration_history.py",
        "scripts/check_permission_routes.py",
    ] {
        run_process(&root, "python", &[script])?;
    }
    Ok(())
}

pub(super) fn run_owned(dir: &Path, executable: &str, args: &[String]) -> Result<()> {
    let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_process(dir, executable, &borrowed)
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
