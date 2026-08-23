use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use crate::{
    Result,
    cli::CheckScope,
    process::{command_output, run as run_process, run_pnpm},
    workspace::root_dir,
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
    "--workspace",
    "--all-targets",
    "--",
    "-D",
    "warnings",
    "-D",
    "clippy::redundant_clone",
];
pub(crate) const WORKSPACE_TEST_ARGS: &[&str] = &["test", "--locked", "--workspace", "--jobs", "2"];
pub(crate) const FEATURE_MATRIX_TARGET_DIR: &str = "target/feature-matrix";

static NEXT_VERIFY_ARTIFACT: AtomicU64 = AtomicU64::new(1);

struct BackendSnapshots {
    openapi: Option<PathBuf>,
    mysql: Option<PathBuf>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsumerContractPlan {
    pub(crate) mode: &'static str,
    pub(crate) backend_commit: String,
    pub(crate) backend_repository: String,
    pub(crate) require_pin: bool,
}

impl Drop for BackendSnapshots {
    fn drop(&mut self) {
        if let Some(path) = &self.openapi {
            let _ = fs::remove_file(path);
        }
        if let Some(path) = &self.mysql {
            let _ = fs::remove_file(path);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FeatureMatrixEntry {
    package: String,
    minimal: Vec<String>,
    maximal: Vec<String>,
    test_targets: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceGraph {
    pub(crate) package_by_dir: BTreeMap<String, String>,
    pub(crate) reverse_dependencies: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FrontendProfile {
    Contract,
    Code,
    Browser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BackendSnapshotProfile {
    OpenApiContract,
    Mysql,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VerifySelection {
    pub(crate) full_reason: Option<String>,
    pub(crate) backend_packages: BTreeSet<String>,
    pub(crate) backend_snapshot_profiles: BTreeSet<BackendSnapshotProfile>,
    pub(crate) frontend_profiles: BTreeSet<FrontendProfile>,
    reasons: Vec<String>,
}

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

fn export_and_verify_backend_snapshots(
    profiles: &BTreeSet<BackendSnapshotProfile>,
) -> Result<BackendSnapshots> {
    let root = root_dir();
    let artifact_dir = root.join("target").join("xtask");
    fs::create_dir_all(&artifact_dir).map_err(|error| {
        format!(
            "无法创建完整门禁临时目录 {}：{error}",
            artifact_dir.display()
        )
    })?;
    let suffix = format!(
        "{}-{}",
        process::id(),
        NEXT_VERIFY_ARTIFACT.fetch_add(1, Ordering::Relaxed)
    );
    let openapi = profiles
        .contains(&BackendSnapshotProfile::OpenApiContract)
        .then(|| artifact_dir.join(format!("verify-{suffix}-openapi.json")));
    let mysql = profiles
        .contains(&BackendSnapshotProfile::Mysql)
        .then(|| artifact_dir.join(format!("verify-{suffix}-mysql.sql")));
    let snapshots = BackendSnapshots { openapi, mysql };

    if let Some(openapi) = &snapshots.openapi {
        run_owned(
            &root,
            "cargo",
            &[
                "run".to_owned(),
                "--locked".to_owned(),
                "-p".to_owned(),
                "ryframe-api".to_owned(),
                "--bin".to_owned(),
                "export_openapi".to_owned(),
                "--".to_owned(),
                openapi.to_string_lossy().into_owned(),
            ],
        )?;
        verify_snapshot(
            "OpenAPI",
            &root.join("openapi").join("openapi.json"),
            openapi,
            "cargo api-sync",
        )?;
    }
    if let Some(mysql) = &snapshots.mysql {
        run_owned(
            &root,
            "cargo",
            &[
                "run".to_owned(),
                "--locked".to_owned(),
                "-p".to_owned(),
                "ryframe-db".to_owned(),
                "--bin".to_owned(),
                "export_mysql_snapshot".to_owned(),
                "--".to_owned(),
                mysql.to_string_lossy().into_owned(),
            ],
        )?;
        verify_snapshot(
            "MySQL 基线",
            &root.join("sql").join("ryframe_config.sql"),
            mysql,
            "cargo run --locked -p ryframe-db --bin export_mysql_snapshot -- sql/ryframe_config.sql",
        )?;
    }
    Ok(snapshots)
}

fn verify_snapshot(
    label: &str,
    committed_path: &Path,
    generated_path: &Path,
    refresh_command: &str,
) -> Result<()> {
    let committed = fs::read(committed_path).map_err(|error| {
        format!(
            "无法读取已提交的{label}快照 {}：{error}",
            committed_path.display()
        )
    })?;
    let generated = fs::read(generated_path).map_err(|error| {
        format!(
            "无法读取本次生成的{label}快照 {}：{error}",
            generated_path.display()
        )
    })?;
    if committed != generated {
        return Err(format!(
            "{label}快照已过期：{} 与当前代码生成结果不同；确认变更后运行 `{refresh_command}`",
            committed_path.display()
        )
        .into());
    }
    println!("{label}快照与当前代码一致。");
    Ok(())
}

fn run_consumer_contract(frontend_dir: &Path, backend_snapshots: &BackendSnapshots) -> Result<()> {
    let openapi = backend_snapshots
        .openapi
        .as_deref()
        .ok_or("消费契约检查缺少本次生成的 OpenAPI 快照")?;
    let candidate = frontend_dir.join("openapi/candidate.json").is_file();
    let candidate_commit = if candidate {
        Some(
            command_output(&root_dir(), "git", &["rev-parse", "HEAD"])?
                .trim()
                .to_owned(),
        )
    } else {
        None
    };
    let plan = load_consumer_contract_plan(frontend_dir, candidate_commit.as_deref())?;
    let arguments = consumer_contract_arguments(&plan, openapi);
    let arguments = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    run_pnpm(frontend_dir, &arguments)
}

pub(crate) fn load_consumer_contract_plan(
    frontend_dir: &Path,
    candidate_commit: Option<&str>,
) -> Result<ConsumerContractPlan> {
    let source_path = frontend_dir.join("openapi").join("source.json");
    let source: serde_json::Value = serde_json::from_slice(
        &fs::read(&source_path)
            .map_err(|error| format!("无法读取契约来源 {}：{error}", source_path.display()))?,
    )
    .map_err(|error| format!("契约来源 {} 不是有效 JSON：{error}", source_path.display()))?;
    let candidate = frontend_dir.join("openapi/candidate.json").is_file();
    consumer_contract_plan(&source, candidate, candidate_commit)
}

pub(crate) fn consumer_contract_plan(
    source: &serde_json::Value,
    candidate: bool,
    candidate_commit: Option<&str>,
) -> Result<ConsumerContractPlan> {
    let backend_repository = source
        .get("backend_repository")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("契约来源缺少 backend_repository")?
        .to_owned();
    let backend_commit = if candidate {
        candidate_commit
            .filter(|value| !value.trim().is_empty())
            .ok_or("候选契约检查缺少当前后端提交")?
            .to_owned()
    } else {
        source
            .get("backend_commit")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or("正式契约来源缺少 backend_commit")?
            .to_owned()
    };
    Ok(ConsumerContractPlan {
        mode: if candidate { "candidate" } else { "formal" },
        backend_commit,
        backend_repository,
        require_pin: !candidate,
    })
}

pub(crate) fn consumer_contract_arguments(
    plan: &ConsumerContractPlan,
    openapi: &Path,
) -> Vec<String> {
    vec![
        "consumer:check".to_owned(),
        "--".to_owned(),
        "--mode".to_owned(),
        plan.mode.to_owned(),
        "--openapi".to_owned(),
        openapi.to_string_lossy().into_owned(),
        "--backend-commit".to_owned(),
        plan.backend_commit.clone(),
        "--backend-repository".to_owned(),
        plan.backend_repository.clone(),
        "--require-pin".to_owned(),
        plan.require_pin.to_string(),
    ]
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

pub(crate) fn changed_paths(repository: &Path) -> Result<Vec<String>> {
    if !repository.is_dir() {
        return Err(format!("Git 工作树不存在：{}", repository.display()).into());
    }
    let tracked = command_output(
        repository,
        "git",
        &["diff", "--name-only", "-z", "HEAD", "--"],
    )?;
    let untracked = command_output(
        repository,
        "git",
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let mut paths = tracked
        .split('\0')
        .chain(untracked.split('\0'))
        .filter(|path| !path.is_empty())
        .map(normalize_relative_path)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn normalize_relative_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub(crate) fn classify_changes(
    backend_paths: &[String],
    frontend_paths: &[String],
    graph: &WorkspaceGraph,
) -> VerifySelection {
    let mut selection = VerifySelection::default();
    for path in backend_paths {
        classify_backend_path(path, graph, &mut selection);
        if selection.full_reason.is_some() {
            return selection;
        }
    }
    for path in frontend_paths {
        classify_frontend_path(path, &mut selection);
        if selection.full_reason.is_some() {
            return selection;
        }
    }
    selection
}

pub(crate) fn complete_verify_selection(selection: &mut VerifySelection, graph: &WorkspaceGraph) {
    selection.backend_packages =
        reverse_dependency_closure(&selection.backend_packages, &graph.reverse_dependencies);
    if selection.backend_packages.contains("ryframe-api") {
        selection
            .backend_snapshot_profiles
            .insert(BackendSnapshotProfile::OpenApiContract);
    }
    if selection.backend_packages.contains("ryframe-db") {
        selection
            .backend_snapshot_profiles
            .insert(BackendSnapshotProfile::Mysql);
    }
}

pub(crate) fn needs_consumer_contract(profiles: &BTreeSet<BackendSnapshotProfile>) -> bool {
    profiles.contains(&BackendSnapshotProfile::OpenApiContract)
}

fn classify_backend_path(path: &str, graph: &WorkspaceGraph, selection: &mut VerifySelection) {
    if is_documentation(path) {
        selection.reasons.push(format!("后端文档：{path}"));
        return;
    }
    if path == "openapi/openapi.json" {
        selection
            .backend_snapshot_profiles
            .insert(BackendSnapshotProfile::OpenApiContract);
        selection.reasons.push(format!("后端 OpenAPI 快照：{path}"));
        return;
    }
    if path == "sql/ryframe_config.sql" {
        selection
            .backend_snapshot_profiles
            .insert(BackendSnapshotProfile::Mysql);
        selection.reasons.push(format!("后端 MySQL 快照：{path}"));
        return;
    }
    if is_backend_shared_path(path) {
        selection.full_reason = Some(format!("后端共享、依赖、CI 或工具链文件发生变化：{path}"));
        return;
    }
    let package = graph
        .package_by_dir
        .iter()
        .filter(|(directory, _)| {
            path == directory.as_str() || path.starts_with(&format!("{directory}/"))
        })
        .max_by_key(|(directory, _)| directory.len())
        .map(|(_, package)| package);
    if let Some(package) = package {
        if path.ends_with("/Cargo.toml") || path.ends_with("/build.rs") {
            selection.full_reason = Some(format!("包依赖或构建配置发生变化：{path}"));
        } else {
            selection.backend_packages.insert(package.clone());
            selection.reasons.push(format!("后端包 {package}：{path}"));
        }
        return;
    }
    selection.full_reason = Some(format!("无法安全分类后端变更：{path}"));
}

fn classify_frontend_path(path: &str, selection: &mut VerifySelection) {
    if is_documentation(path) {
        selection.reasons.push(format!("前端文档：{path}"));
        return;
    }
    if is_frontend_shared_path(path) {
        selection.full_reason = Some(format!("前端共享、依赖、CI 或工具链文件发生变化：{path}"));
        return;
    }
    if path.starts_with("openapi/") || path.starts_with("src/api/generated/") {
        selection
            .frontend_profiles
            .insert(FrontendProfile::Contract);
        selection.reasons.push(format!("前端契约：{path}"));
    } else if path.starts_with("tests/e2e/") || path.starts_with("tests/browser/") {
        selection.frontend_profiles.insert(FrontendProfile::Browser);
        selection.reasons.push(format!("浏览器测试：{path}"));
    } else if path.starts_with("src/")
        || path.starts_with("tests/unit/")
        || path.starts_with("public/")
    {
        selection.frontend_profiles.insert(FrontendProfile::Code);
        selection.reasons.push(format!("前端代码：{path}"));
    } else {
        selection.full_reason = Some(format!("无法安全分类前端变更：{path}"));
    }
}

fn is_documentation(path: &str) -> bool {
    matches!(
        path,
        "README.md"
            | "ARCHITECTURE.md"
            | "LICENSE"
            | "LICENSE.md"
            | "CHANGELOG.md"
            | "CONTRIBUTING.md"
            | "SECURITY.md"
            | "CODE_OF_CONDUCT.md"
            | "docs/api.md"
            | "docs/architecture.md"
            | "docs/data.md"
            | "docs/development.md"
            | "docs/operations.md"
    )
}

fn is_backend_shared_path(path: &str) -> bool {
    path == "Cargo.toml"
        || path == "Cargo.lock"
        || path.starts_with(".cargo/")
        || path.starts_with(".github/")
        || path.starts_with("catalog/")
        || path.starts_with("config/")
        || path.starts_with("openapi/")
        || path.starts_with("scripts/")
        || path.starts_with("sql/")
        || path.starts_with("xtask/")
        || path.starts_with("rust-toolchain")
        || matches!(path, ".gitignore" | "deny.toml")
}

fn is_frontend_shared_path(path: &str) -> bool {
    path == "package.json"
        || path == "pnpm-lock.yaml"
        || path == "openapi/source.json"
        || path.starts_with(".github/")
        || path.starts_with("scripts/")
        || path.starts_with("tsconfig")
        || path.contains(".config.")
        || matches!(path, ".gitignore" | "eslint.config.js" | "qodana.yaml")
}

pub(crate) fn reverse_dependency_closure(
    initial: &BTreeSet<String>,
    reverse_dependencies: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut selected = initial.clone();
    let mut queue = initial.iter().cloned().collect::<VecDeque<_>>();
    while let Some(package) = queue.pop_front() {
        if let Some(dependents) = reverse_dependencies.get(&package) {
            for dependent in dependents {
                if selected.insert(dependent.clone()) {
                    queue.push_back(dependent.clone());
                }
            }
        }
    }
    selected
}

fn print_selection(selection: &VerifySelection) {
    for reason in &selection.reasons {
        println!("变更分类：{reason}");
    }
    if !selection.backend_packages.is_empty() {
        println!(
            "后端检查包（含反向依赖）：{}",
            selection
                .backend_packages
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !selection.backend_snapshot_profiles.is_empty() {
        let labels = selection
            .backend_snapshot_profiles
            .iter()
            .map(|profile| match profile {
                BackendSnapshotProfile::OpenApiContract => "OpenAPI 与消费契约",
                BackendSnapshotProfile::Mysql => "MySQL 基线",
            })
            .collect::<Vec<_>>();
        println!("后端快照画像：{}", labels.join("、"));
    }
    if !selection.frontend_profiles.is_empty() {
        let labels = selection
            .frontend_profiles
            .iter()
            .map(|profile| match profile {
                FrontendProfile::Contract => "契约",
                FrontendProfile::Code => "代码",
                FrontendProfile::Browser => "浏览器",
            })
            .collect::<Vec<_>>();
        println!("前端检查画像：{}", labels.join("、"));
    }
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

fn run_owned(dir: &Path, executable: &str, args: &[String]) -> Result<()> {
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

pub(crate) fn frontend_profile_commands(
    profiles: &BTreeSet<FrontendProfile>,
    consumer_contract_ran: bool,
) -> Vec<&'static str> {
    let contract = profiles.contains(&FrontendProfile::Contract);
    let code = profiles.contains(&FrontendProfile::Code);
    let mut commands = Vec::new();
    if contract && !consumer_contract_ran {
        // api:check 同时覆盖来源摘要、契约结构、派生物与 operation 使用，避免拆分后漏项。
        commands.push("api:check");
    }
    if code {
        commands.extend(["check:source-size", "lint", "lint:styles"]);
    }
    if contract || code {
        if code && !contract && !consumer_contract_ran {
            commands.push("check:api-operations");
        }
        if !consumer_contract_ran {
            commands.extend(["typecheck", "test:unit"]);
        }
        commands.push("build");
    }
    if code {
        commands.push("check:bundle");
    }
    if profiles.contains(&FrontendProfile::Browser) {
        commands.push("test:browser-smoke");
    }
    commands
}

pub(crate) fn feature_matrix() -> Result<()> {
    let root = root_dir();
    let metadata = load_workspace_metadata(&root)?;
    let registry = load_feature_registry(&root)?;
    validate_feature_registry(&metadata, &registry)?;

    for entry in &registry {
        // 默认 Workspace 门禁已经执行默认 feature 的 Clippy 与测试；最小组合只需证明
        // 全目标可编译，最大组合再覆盖 feature 专属 lint。
        run_feature_operations(&root, &entry.package, "最小", &entry.minimal, &["check"])?;
        if entry.minimal != entry.maximal {
            run_feature_operations(
                &root,
                &entry.package,
                "最大",
                &entry.maximal,
                &["check", "clippy"],
            )?;
        }
        // 紧跟同一最大组合链接明确指定的测试目标，避免切换到其他包的 feature 后
        // 再次重建共享依赖。默认测试仍由 Workspace 门禁负责。
        run_feature_tests(&root, entry)?;
    }
    Ok(())
}

fn check_feature_registry(root: &Path) -> Result<()> {
    let metadata = load_workspace_metadata(root)?;
    let registry = load_feature_registry(root)?;
    validate_feature_registry(&metadata, &registry)?;
    println!("Cargo feature 注册表检查通过。");
    Ok(())
}

fn load_feature_registry(root: &Path) -> Result<Vec<FeatureMatrixEntry>> {
    let path = root.join("config/feature-matrix.json");
    let source = fs::read(&path)
        .map_err(|error| format!("无法读取 feature 注册表 {}：{error}", path.display()))?;
    let document: serde_json::Value = serde_json::from_slice(&source)
        .map_err(|error| format!("feature 注册表不是有效 JSON：{error}"))?;
    if document.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err("feature 注册表 version 必须为 1".into());
    }
    let packages = document
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("feature 注册表缺少 packages 数组")?;

    packages
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let package = entry
                .get("package")
                .and_then(serde_json::Value::as_str)
                .filter(|package| !package.trim().is_empty())
                .ok_or_else(|| format!("feature 注册表 packages[{index}] 缺少 package"))?;
            Ok(FeatureMatrixEntry {
                package: package.to_owned(),
                minimal: feature_names(entry.get("minimal"), index, "minimal")?,
                maximal: feature_names(entry.get("maximal"), index, "maximal")?,
                test_targets: feature_names(entry.get("test_targets"), index, "test_targets")?,
            })
        })
        .collect()
}

fn feature_names(
    value: Option<&serde_json::Value>,
    index: usize,
    field: &str,
) -> Result<Vec<String>> {
    value
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("feature 注册表 packages[{index}].{field} 必须是数组"))?
        .iter()
        .enumerate()
        .map(|(feature_index, value)| {
            value
                .as_str()
                .filter(|feature| !feature.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "feature 注册表 packages[{index}].{field}[{feature_index}] 必须是非空字符串"
                    )
                    .into()
                })
        })
        .collect()
}

fn load_workspace_metadata(root: &Path) -> Result<serde_json::Value> {
    let output = command_output(
        root,
        "cargo",
        &["metadata", "--format-version", "1", "--no-deps"],
    )?;
    serde_json::from_str(&output)
        .map_err(|error| format!("Cargo 工作区元数据不是有效 JSON：{error}").into())
}

pub(crate) fn load_workspace_graph(root: &Path) -> Result<WorkspaceGraph> {
    let metadata = load_workspace_metadata(root)?;
    let members = metadata
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo 元数据缺少 workspace_members")?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<BTreeSet<_>>();
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo 元数据缺少 packages")?;
    let mut package_by_dir = BTreeMap::new();
    let mut dependency_names = BTreeMap::<String, BTreeSet<String>>::new();
    let mut workspace_names = BTreeSet::new();

    for package in packages {
        let id = package
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or("Cargo package 缺少 id")?;
        if !members.contains(id) {
            continue;
        }
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or("Cargo package 缺少 name")?
            .to_owned();
        let manifest = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .ok_or("Cargo package 缺少 manifest_path")?;
        let directory = PathBuf::from(manifest)
            .parent()
            .ok_or("Cargo manifest_path 没有父目录")?
            .strip_prefix(root)
            .map_err(|_| format!("Cargo 包 {name} 不属于当前工作区"))?
            .to_string_lossy()
            .replace('\\', "/");
        if package_by_dir.insert(directory, name.clone()).is_some() {
            return Err(format!("Cargo 工作区存在重复包目录：{name}").into());
        }
        workspace_names.insert(name.clone());
        let dependencies = package
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .ok_or("Cargo package 缺少 dependencies")?
            .iter()
            .filter_map(|dependency| dependency.get("name"))
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
        dependency_names.insert(name, dependencies);
    }

    let mut reverse_dependencies = BTreeMap::<String, BTreeSet<String>>::new();
    for (package, dependencies) in dependency_names {
        for dependency in dependencies {
            if workspace_names.contains(&dependency) {
                reverse_dependencies
                    .entry(dependency)
                    .or_default()
                    .insert(package.clone());
            }
        }
    }
    Ok(WorkspaceGraph {
        package_by_dir,
        reverse_dependencies,
    })
}

fn workspace_features(metadata: &serde_json::Value) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let members = metadata
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo 元数据缺少 workspace_members")?
        .iter()
        .map(|member| {
            member
                .as_str()
                .map(str::to_owned)
                .ok_or("Cargo workspace_members 中存在非字符串成员")
        })
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo 元数据缺少 packages")?;
    let mut result = BTreeMap::new();

    for package in packages {
        let id = package
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or("Cargo package 缺少 id")?;
        if !members.contains(id) {
            continue;
        }
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or("Cargo package 缺少 name")?;
        let features = package
            .get("features")
            .and_then(serde_json::Value::as_object)
            .ok_or("Cargo package 缺少 features")?
            .keys()
            .cloned()
            .collect();
        if result.insert(name.to_owned(), features).is_some() {
            return Err(format!("Cargo 工作区存在重复包名：{name}").into());
        }
    }

    if result.len() != members.len() {
        return Err("Cargo 元数据未包含全部工作区成员".into());
    }
    Ok(result)
}

fn validate_feature_registry(
    metadata: &serde_json::Value,
    registry: &[FeatureMatrixEntry],
) -> Result<()> {
    let packages = workspace_features(metadata)?;
    let feature_packages = packages
        .iter()
        .filter(|(_, features)| !features.is_empty())
        .map(|(name, _)| name.as_str())
        .collect::<BTreeSet<_>>();
    let mut registered_packages = BTreeSet::new();

    for entry in registry {
        if !registered_packages.insert(entry.package.as_str()) {
            return Err(format!("Cargo feature 注册表重复登记包：{}", entry.package).into());
        }
        let available = packages
            .get(entry.package.as_str())
            .ok_or_else(|| format!("Cargo feature 注册表包含未知包：{}", entry.package))?;
        if available.is_empty() {
            return Err(format!("包 {} 没有 feature，不应登记矩阵", entry.package).into());
        }

        let minimal =
            validate_feature_combination(&entry.package, "最小", &entry.minimal, available)?;
        let maximal =
            validate_feature_combination(&entry.package, "最大", &entry.maximal, available)?;
        if !minimal.is_subset(&maximal) {
            return Err(
                format!("包 {} 的最小 feature 组合不是最大组合的子集", entry.package).into(),
            );
        }
        if &maximal != available {
            let missing = available.difference(&maximal).cloned().collect::<Vec<_>>();
            return Err(format!(
                "包 {} 的最大 feature 组合未覆盖：{}",
                entry.package,
                missing.join(", ")
            )
            .into());
        }
        let test_targets = entry.test_targets.iter().collect::<BTreeSet<_>>();
        if test_targets.len() != entry.test_targets.len() {
            return Err(format!("包 {} 的 feature 测试目标包含重复项", entry.package).into());
        }
    }

    if registered_packages != feature_packages {
        let missing = feature_packages
            .difference(&registered_packages)
            .copied()
            .collect::<Vec<_>>();
        let extra = registered_packages
            .difference(&feature_packages)
            .copied()
            .collect::<Vec<_>>();
        return Err(format!(
            "Cargo feature 注册表与工作区不一致；未登记：[{}]，多余：[{}]",
            missing.join(", "),
            extra.join(", ")
        )
        .into());
    }
    Ok(())
}

pub(crate) fn validate_feature_combination(
    package: &str,
    label: &str,
    combination: &[String],
    available: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let selected = combination.iter().cloned().collect::<BTreeSet<_>>();
    if selected.len() != combination.len() {
        return Err(format!("包 {package} 的{label} feature 组合包含重复项").into());
    }
    let unknown = selected.difference(available).cloned().collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(format!(
            "包 {package} 的{label} feature 组合包含未知项：{}",
            unknown.join(", ")
        )
        .into());
    }
    Ok(selected)
}

fn run_feature_operations(
    root: &Path,
    package: &str,
    label: &str,
    features: &[String],
    operations: &[&str],
) -> Result<()> {
    println!("检查 {package} 的{label} feature 组合。");
    for operation in operations {
        let args = feature_operation_args(operation, package, features);
        run_owned(root, "cargo", &args)?;
    }
    Ok(())
}

fn run_feature_tests(root: &Path, entry: &FeatureMatrixEntry) -> Result<()> {
    for target in &entry.test_targets {
        println!(
            "检查 {} 的最大 feature 测试目标 {}。",
            entry.package, target
        );
        run_owned(
            root,
            "cargo",
            &feature_test_args(&entry.package, &entry.maximal, target),
        )?;
    }
    Ok(())
}

pub(crate) fn feature_test_args(package: &str, features: &[String], target: &str) -> Vec<String> {
    let mut args = vec![
        "test".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        FEATURE_MATRIX_TARGET_DIR.to_owned(),
        "-p".to_owned(),
        package.to_owned(),
        "--no-default-features".to_owned(),
        "--jobs".to_owned(),
        "2".to_owned(),
        "--test".to_owned(),
        target.to_owned(),
    ];
    if !features.is_empty() {
        args.extend(["--features".to_owned(), features.join(",")]);
    }
    args
}

pub(crate) fn feature_operation_args(
    operation: &str,
    package: &str,
    features: &[String],
) -> Vec<String> {
    let mut args = vec![
        operation.to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        FEATURE_MATRIX_TARGET_DIR.to_owned(),
        "-p".to_owned(),
        package.to_owned(),
        "--no-default-features".to_owned(),
    ];
    args.push("--all-targets".to_owned());
    if !features.is_empty() {
        args.extend(["--features".to_owned(), features.join(",")]);
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
    args
}
