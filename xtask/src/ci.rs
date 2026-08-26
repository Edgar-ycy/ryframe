use std::{
    collections::BTreeSet,
    env,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use crate::{
    Result,
    check::{
        BACKEND_CI_TARGET_DIR, BackendSnapshotProfile, RESOURCE_CI_TARGET_DIR, VerifySelection,
        changed_paths, changed_paths_between, ci_consumer_contract, ci_rust_gate,
        ci_test_jobs_from, classify_changes, complete_verify_selection, load_workspace_graph,
        resource_workspace_compilation,
    },
    cli::CiCommand,
    process::{command_output, run as run_process, run_owned},
    workspace::root_dir,
};

const FULL_CI_EVENTS: &[&str] = &["push", "schedule", "workflow_dispatch"];
const INTEGRATION_PACKAGES: &[&str] = &["ryframe-adapters", "ryframe-db", "ryframe-tenant-db"];
const WINDOWS_RUST_GATE_PROFILE: &str = "windows-smoke";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CiPlan {
    pub(crate) preflight: bool,
    pub(crate) rust_gate: bool,
    pub(crate) integration: bool,
    pub(crate) consumer_contract: bool,
}

impl CiPlan {
    const fn full() -> Self {
        Self {
            preflight: true,
            rust_gate: true,
            integration: true,
            consumer_contract: true,
        }
    }
}

pub(crate) fn run(command: CiCommand, frontend_dir: &Path) -> Result<()> {
    match command {
        CiCommand::Plan => plan(),
        CiCommand::Preflight => preflight(),
        CiCommand::RustGate => rust_gate(frontend_dir),
        CiCommand::Integration => integration(),
        CiCommand::ConsumerContract => consumer_contract(frontend_dir),
    }
}

fn rust_gate(frontend_dir: &Path) -> Result<()> {
    match env::var("RYFRAME_CI_RUST_GATE_PROFILE").ok().as_deref() {
        None | Some("") | Some("standard") => {
            verify_frontend_checkout_from_environment(frontend_dir)?;
            ci_rust_gate(frontend_dir)
        }
        Some(WINDOWS_RUST_GATE_PROFILE) => windows_smoke(frontend_dir),
        Some(profile) => Err(format!(
            "RYFRAME_CI_RUST_GATE_PROFILE 只允许 standard 或 {WINDOWS_RUST_GATE_PROFILE}，实际为 {profile}"
        )
        .into()),
    }
}

fn consumer_contract(frontend_dir: &Path) -> Result<()> {
    verify_frontend_checkout_from_environment(frontend_dir)?;
    verify_formal_contract_source_from_environment(frontend_dir)?;
    ci_consumer_contract(frontend_dir)
}

fn plan() -> Result<()> {
    let root = root_dir();
    let event = env::var("GITHUB_EVENT_NAME").unwrap_or_else(|_| "local".to_owned());
    let action = env::var("GITHUB_EVENT_ACTION").unwrap_or_default();
    let paths = ci_changed_paths(&root, &event)?;
    let graph = load_workspace_graph(&root)?;
    let mut selection = classify_changes(&paths, &[], &graph);
    complete_verify_selection(&mut selection, &graph);
    let plan = ci_plan_for(&event, &action, &selection);

    println!("CI 事件：{event}{}", action_label(&action));
    if paths.is_empty() {
        println!("CI 变更：没有检测到文件差异。");
    } else {
        println!("CI 变更：{}", paths.join(", "));
    }
    if let Some(reason) = &selection.full_reason {
        println!("CI 计划扩大为完整门禁：{reason}");
    }
    for (name, enabled) in plan_outputs(plan) {
        println!("{name}={enabled}");
    }
    write_github_outputs(plan)
}

fn action_label(action: &str) -> String {
    if action.is_empty() {
        String::new()
    } else {
        format!("（{action}）")
    }
}

pub(crate) fn ci_plan_for(event: &str, action: &str, selection: &VerifySelection) -> CiPlan {
    if event == "pull_request" && action == "edited" {
        return CiPlan {
            preflight: false,
            rust_gate: false,
            integration: false,
            consumer_contract: true,
        };
    }
    if FULL_CI_EVENTS.contains(&event) {
        return CiPlan {
            consumer_contract: false,
            ..CiPlan::full()
        };
    }
    if selection.full_reason.is_some() {
        return CiPlan::full();
    }

    let has_backend_work =
        !selection.backend_packages.is_empty() || !selection.backend_snapshot_profiles.is_empty();
    let integration = selection
        .backend_packages
        .iter()
        .any(|package| INTEGRATION_PACKAGES.contains(&package.as_str()));
    let consumer_contract = selection
        .backend_snapshot_profiles
        .contains(&BackendSnapshotProfile::OpenApiContract);
    CiPlan {
        // 文档变更仍执行仓库策略与格式检查，保持 Required 的确定性。
        preflight: true,
        rust_gate: has_backend_work,
        integration,
        consumer_contract,
    }
}

fn ci_changed_paths(root: &Path, event: &str) -> Result<Vec<String>> {
    if let Ok(configured) = env::var("RYFRAME_CI_CHANGED_PATHS") {
        return Ok(parse_changed_paths(&configured));
    }
    if FULL_CI_EVENTS.contains(&event) {
        // 定时、手动和主分支构建必须完整执行，具体文件列表不影响计划。
        return Ok(Vec::new());
    }
    let base = env::var("RYFRAME_CI_BASE_SHA")
        .or_else(|_| env::var("GITHUB_BASE_SHA"))
        .ok();
    let head = env::var("RYFRAME_CI_HEAD_SHA")
        .or_else(|_| env::var("GITHUB_SHA"))
        .ok();
    match (base.as_deref(), head.as_deref()) {
        (Some(base), Some(head)) if valid_git_sha(base) && valid_git_sha(head) => {
            changed_paths_between(root, base, head)
        }
        _ => changed_paths(root),
    }
}

pub(crate) fn parse_changed_paths(value: &str) -> Vec<String> {
    value
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| path.replace('\\', "/"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn valid_git_sha(value: &str) -> bool {
    value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn plan_outputs(plan: CiPlan) -> [(&'static str, bool); 4] {
    [
        ("preflight", plan.preflight),
        ("rust_gate", plan.rust_gate),
        ("integration", plan.integration),
        ("consumer_contract", plan.consumer_contract),
    ]
}

fn write_github_outputs(plan: CiPlan) -> Result<()> {
    let Some(path) = env::var_os("GITHUB_OUTPUT").map(PathBuf::from) else {
        return Ok(());
    };
    let mut output = OpenOptions::new().create(true).append(true).open(&path)?;
    for (name, enabled) in plan_outputs(plan) {
        writeln!(output, "{name}={enabled}")?;
    }
    Ok(())
}

fn preflight() -> Result<()> {
    let root = root_dir();
    run_process(&root, "cargo", &["fmt", "--all", "--", "--check"])?;
    run_process(
        &root,
        "python",
        &[
            "-m",
            "unittest",
            "discover",
            "-s",
            "scripts/tests",
            "-p",
            "test_*.py",
        ],
    )?;
    for script in [
        "scripts/check_prerelease_dependencies.py",
        "scripts/check_supply_chain.py",
        "scripts/check_architecture.py",
        "scripts/check_permission_routes.py",
        "scripts/check_deployment_assets.py",
    ] {
        run_process(&root, "python", &[script])?;
    }
    let migration_args = preflight_migration_args(
        env::var("RYFRAME_CI_BASE_SHA")
            .or_else(|_| env::var("GITHUB_BASE_SHA"))
            .ok()
            .as_deref(),
    );
    run_owned(&root, "python", &migration_args)
}

pub(crate) fn preflight_migration_args(base: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "scripts/check_migration_history.py".to_owned(),
        "--require-frozen".to_owned(),
    ];
    if let Some(base) = base.filter(|base| valid_git_sha(base)) {
        args.extend(["--trusted-ref".to_owned(), base.to_owned()]);
    }
    args
}

fn windows_smoke(frontend_dir: &Path) -> Result<()> {
    verify_frontend_checkout_from_environment(frontend_dir)?;
    let root = root_dir();
    let jobs = ci_test_jobs_from(
        env::var("RYFRAME_CI_TEST_JOBS").ok().as_deref(),
        true,
        std::thread::available_parallelism().map_or(4, usize::from),
    )?;
    for package in ["ryframe", "xtask"] {
        run_owned(&root, "cargo", &windows_check_args(package))?;
    }
    run_owned(&root, "cargo", &windows_process_test_args(jobs))?;
    resource_workspace_compilation(&root, frontend_dir, RESOURCE_CI_TARGET_DIR, jobs)
}

pub(crate) fn windows_check_args(package: &str) -> Vec<String> {
    [
        "check",
        "--locked",
        "--target-dir",
        BACKEND_CI_TARGET_DIR,
        "-p",
        package,
        "--all-targets",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

pub(crate) fn windows_process_test_args(jobs: usize) -> Vec<String> {
    [
        "test".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        BACKEND_CI_TARGET_DIR.to_owned(),
        "-p".to_owned(),
        "xtask".to_owned(),
        "--test".to_owned(),
        "process_windows".to_owned(),
        "--jobs".to_owned(),
        jobs.max(1).to_string(),
        "--".to_owned(),
        "--nocapture".to_owned(),
    ]
    .into_iter()
    .collect()
}

fn verify_frontend_checkout_from_environment(frontend_dir: &Path) -> Result<()> {
    let Some(requested_ref) = env::var("RYFRAME_CI_FRONTEND_REF")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(());
    };
    let resolved = command_output(frontend_dir, "git", &["rev-parse", "--verify", "HEAD"])?;
    verify_frontend_checkout_ref(&requested_ref, resolved.trim())?;
    println!("前端 Workspace 固定为 {}。", resolved.trim());
    Ok(())
}

pub(crate) fn verify_frontend_checkout_ref(requested_ref: &str, resolved: &str) -> Result<()> {
    if !valid_git_sha(resolved) {
        return Err("前端实际检出提交不是 40 位 Git SHA".into());
    }
    if valid_git_sha(requested_ref) && !requested_ref.eq_ignore_ascii_case(resolved) {
        return Err(format!(
            "前端实际检出提交与请求 SHA 不一致：期望 {requested_ref}，实际 {resolved}"
        )
        .into());
    }
    Ok(())
}

fn verify_formal_contract_source_from_environment(frontend_dir: &Path) -> Result<()> {
    let Some(backend_head) = env::var("RYFRAME_CI_BACKEND_HEAD")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(());
    };
    if !valid_git_sha(&backend_head) {
        return Err("RYFRAME_CI_BACKEND_HEAD 必须是 40 位 Git SHA".into());
    }
    let repository = env::var("RYFRAME_CI_BACKEND_REPOSITORY")
        .unwrap_or_else(|_| "Edgar-ycy/ryframe".to_owned());
    let candidate = env::var_os("RYFRAME_CI_CANDIDATE_OPENAPI")
        .map(PathBuf::from)
        .ok_or("消费契约来源检查缺少 RYFRAME_CI_CANDIDATE_OPENAPI")?;
    let root = root_dir();
    run_owned(
        &root,
        "python",
        &formal_contract_source_args(frontend_dir, &backend_head, &repository, &candidate),
    )
}

pub(crate) fn formal_contract_source_args(
    frontend_dir: &Path,
    backend_head: &str,
    backend_repository: &str,
    candidate: &Path,
) -> Vec<String> {
    vec![
        "scripts/verify_frontend_contract_source.py".to_owned(),
        "--backend-worktree".to_owned(),
        ".".to_owned(),
        "--backend-head".to_owned(),
        backend_head.to_owned(),
        "--backend-repository".to_owned(),
        backend_repository.to_owned(),
        "--source-metadata".to_owned(),
        frontend_dir
            .join("openapi/source.json")
            .to_string_lossy()
            .into_owned(),
        "--frontend-openapi".to_owned(),
        frontend_dir
            .join("openapi/openapi.json")
            .to_string_lossy()
            .into_owned(),
        "--candidate-openapi".to_owned(),
        candidate.to_string_lossy().into_owned(),
    ]
}

fn integration() -> Result<()> {
    let root = root_dir();
    let jobs = ci_test_jobs_from(
        env::var("RYFRAME_CI_TEST_JOBS").ok().as_deref(),
        cfg!(windows),
        std::thread::available_parallelism().map_or(1, usize::from),
    )?;
    for (package, target) in [
        ("ryframe-db", "mysql_real_protocol"),
        ("ryframe-adapters", "redis_real_protocol"),
    ] {
        run_owned(
            &root,
            "cargo",
            &integration_test_args(package, target, jobs),
        )?;
    }
    Ok(())
}

pub(crate) fn integration_test_args(package: &str, target: &str, jobs: usize) -> Vec<String> {
    [
        "test".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        BACKEND_CI_TARGET_DIR.to_owned(),
        "-p".to_owned(),
        package.to_owned(),
        "--test".to_owned(),
        target.to_owned(),
        "--jobs".to_owned(),
        jobs.max(1).to_string(),
        "--".to_owned(),
        "--nocapture".to_owned(),
    ]
    .into_iter()
    .collect()
}
