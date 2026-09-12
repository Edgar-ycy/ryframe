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
        BACKEND_CI_TARGET_DIR, RESOURCE_CI_TARGET_DIR, VerifySelection, changed_paths,
        changed_paths_between, ci_target_policy, ci_test_jobs_from, classify_changes,
        complete_verify_selection, load_workspace_graph, resource_workspace_compilation,
    },
    cli::{CiCommand, ResourceGateReplayOptions},
    contract::verify_contract_source,
    process::{command_output, run_owned},
    workspace::root_dir,
};

#[path = "ci/required.rs"]
mod required;
#[path = "ci/resource_gate.rs"]
pub(crate) mod resource_gate;
#[path = "ci/security.rs"]
mod security;
#[path = "ci/task_plan.rs"]
mod task_plan;

#[allow(unused_imports)]
pub(crate) use required::validate_required_jobs;
#[allow(unused_imports)]
pub(crate) use security::source_command as security_source_command;
#[allow(unused_imports)]
pub(crate) use task_plan::{
    CiJob, ci_execution_plan_for, ci_execution_plan_for_profile, ci_plan_for, plan_outputs,
    required_ci_jobs,
};

const FULL_CI_EVENTS: &[&str] = &["push", "schedule", "workflow_dispatch"];
const INTEGRATION_PACKAGES: &[&str] = &["ryframe-adapters", "ryframe-db", "ryframe-tenant-db"];
const WINDOWS_RUST_GATE_PROFILE: &str = "windows-smoke";

pub(crate) fn run(command: CiCommand, frontend_dir: &Path) -> Result<()> {
    match command {
        CiCommand::Plan => plan(),
        CiCommand::ResourceGateReplay(options) => resource_gate_replay(&options, frontend_dir),
        command => execute_ci_command(&command, frontend_dir),
    }
}

fn execute_ci_command(command: &CiCommand, frontend_dir: &Path) -> Result<()> {
    let plan = ci_execution_plan_for(command)?;
    task_plan::execute_ci_job(command, &plan, frontend_dir)
}

fn resource_gate_replay(options: &ResourceGateReplayOptions, frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let arguments = resource_gate_replay_args(options, &root, frontend_dir)?;
    run_owned(&root, "python", &arguments)
}

pub(crate) fn resource_gate_replay_args(
    options: &ResourceGateReplayOptions,
    backend: &Path,
    frontend: &Path,
) -> Result<Vec<String>> {
    fn utf8(path: &Path) -> Result<String> {
        path.to_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| "resource gate replay 路径必须能表示为 UTF-8".into())
    }

    let mut arguments = vec![
        "scripts/resource_gate_replay.py".to_owned(),
        "--repository".to_owned(),
        utf8(backend)?,
        "--frontend-repository".to_owned(),
        utf8(frontend)?,
        "--manifest".to_owned(),
        utf8(&options.manifest)?,
        "--work-dir".to_owned(),
        utf8(&options.work_dir)?,
        "--report".to_owned(),
        utf8(&options.report)?,
    ];
    if options.activation_gate {
        arguments.push("--activation-gate".to_owned());
    }
    Ok(arguments)
}

fn plan() -> Result<()> {
    let root = root_dir();
    let event = env::var("GITHUB_EVENT_NAME").unwrap_or_else(|_| "local".to_owned());
    let action = env::var("GITHUB_EVENT_ACTION").unwrap_or_default();
    let (paths, repository_range_valid) = ci_changed_paths(&root, &event)?;
    let graph = load_workspace_graph(&root)?;
    let selection = ci_selection_for_paths(&paths, &graph);
    let resource_gate = resource_gate::should_run_for_paths(&paths)
        || resource_gate_required_for_ci_range(&event, repository_range_valid);
    let plan = ci_plan_for(&event, &action, &selection, resource_gate)?;

    println!("CI 事件：{event}{}", action_label(&action));
    if paths.is_empty() {
        println!("CI 变更：没有检测到文件差异。");
    } else {
        println!("CI 变更：{}", paths.join(", "));
    }
    if let Some(reason) = &selection.full_reason {
        println!("CI 计划扩大为完整门禁：{reason}");
    }
    for (name, enabled) in plan_outputs(&plan) {
        println!("{name}={enabled}");
    }
    write_github_outputs(&plan)
}

pub(crate) fn ci_selection_for_paths(
    paths: &[String],
    graph: &crate::check::WorkspaceGraph,
) -> VerifySelection {
    if let Some(reason) = resource_gate::full_fallback_reason_for_paths(paths) {
        let mut selection = VerifySelection::default();
        selection.full_reason = Some(reason);
        return selection;
    }
    let generic_paths = paths
        .iter()
        .filter(|path| !resource_gate::delegates_generic_ci_path(path))
        .cloned()
        .collect::<Vec<_>>();
    let mut selection = classify_changes(&generic_paths, &[], graph);
    complete_verify_selection(&mut selection, graph);
    selection
}

fn action_label(action: &str) -> String {
    if action.is_empty() {
        String::new()
    } else {
        format!("（{action}）")
    }
}

fn ci_changed_paths(root: &Path, event: &str) -> Result<(Vec<String>, bool)> {
    if let Ok(configured) = env::var("RYFRAME_CI_CHANGED_PATHS") {
        return Ok((
            parse_changed_paths(&configured),
            configured_ci_range_valid(),
        ));
    }
    if FULL_CI_EVENTS.contains(&event) {
        // 定时、手动和主分支构建必须完整执行，具体文件列表不影响计划。
        return Ok((Vec::new(), configured_ci_range_valid()));
    }
    let base = env::var("RYFRAME_CI_BASE_SHA")
        .or_else(|_| env::var("GITHUB_BASE_SHA"))
        .ok();
    let head = env::var("RYFRAME_CI_HEAD_SHA")
        .or_else(|_| env::var("GITHUB_SHA"))
        .ok();
    match (base.as_deref(), head.as_deref()) {
        (Some(base), Some(head)) if valid_git_sha(base) && valid_git_sha(head) => {
            match changed_paths_between(root, base, head) {
                Ok(paths) => Ok((paths, true)),
                Err(error) => {
                    eprintln!("CI Git 范围不可解析，将由 resource-gate 执行完整回退：{error}");
                    Ok((changed_paths(root)?, false))
                }
            }
        }
        _ => Ok((changed_paths(root)?, false)),
    }
}

fn configured_ci_range_valid() -> bool {
    let base = env::var("RYFRAME_CI_BASE_SHA")
        .or_else(|_| env::var("GITHUB_BASE_SHA"))
        .ok();
    let head = env::var("RYFRAME_CI_HEAD_SHA")
        .or_else(|_| env::var("GITHUB_SHA"))
        .ok();
    matches!(
        (base.as_deref(), head.as_deref()),
        (Some(base), Some(head)) if valid_git_sha(base) && valid_git_sha(head)
    )
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

pub(crate) fn resource_gate_required_for_ci_range(
    event: &str,
    repository_range_valid: bool,
) -> bool {
    event == "pull_request" && !repository_range_valid
}

fn write_github_outputs(plan: &[CiJob]) -> Result<()> {
    let Some(path) = env::var_os("GITHUB_OUTPUT").map(PathBuf::from) else {
        return Ok(());
    };
    let mut output = OpenOptions::new().create(true).append(true).open(&path)?;
    for (name, enabled) in plan_outputs(plan) {
        writeln!(output, "{name}={enabled}")?;
    }
    Ok(())
}

fn windows_smoke(frontend_dir: &Path) -> Result<()> {
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
    let repository = env::var("RYFRAME_CI_BACKEND_REPOSITORY")
        .unwrap_or_else(|_| "Edgar-ycy/ryframe".to_owned());
    let candidate = env::var_os("RYFRAME_CI_CANDIDATE_OPENAPI")
        .map(PathBuf::from)
        .ok_or("消费契约来源检查缺少 RYFRAME_CI_CANDIDATE_OPENAPI")?;
    let root = root_dir();
    let commit = verify_contract_source(
        &root,
        &backend_head,
        &repository,
        &frontend_dir.join("openapi/source.json"),
        &frontend_dir.join("openapi/openapi.json"),
        &candidate,
    )?;
    println!("{commit}");
    Ok(())
}

fn integration() -> Result<()> {
    let root = root_dir();
    let targets = ci_target_policy()?;
    let jobs = ci_test_jobs_from(
        env::var("RYFRAME_CI_TEST_JOBS").ok().as_deref(),
        cfg!(windows),
        std::thread::available_parallelism().map_or(1, usize::from),
    )?;
    for (package, target, features) in [
        (
            "ryframe-db",
            "mysql_real_protocol",
            Some("repositories,migration"),
        ),
        ("ryframe-adapters", "redis_real_protocol", Some("redis-api")),
    ] {
        run_owned(
            &root,
            "cargo",
            &integration_test_args_for_target(package, target, features, &targets.backend, jobs),
        )?;
    }
    run_owned(
        &root,
        "python",
        &tls_integration_args(&targets.backend, jobs),
    )?;
    Ok(())
}

pub(crate) fn tls_integration_args(target_dir: &str, jobs: usize) -> Vec<String> {
    vec![
        "scripts/tls_integration_gate.py".to_owned(),
        "--backend-root".to_owned(),
        ".".to_owned(),
        "--target-dir".to_owned(),
        target_dir.to_owned(),
        "--jobs".to_owned(),
        jobs.max(1).to_string(),
    ]
}

pub(crate) fn integration_test_args_for_target(
    package: &str,
    target: &str,
    features: Option<&str>,
    target_dir: &str,
    jobs: usize,
) -> Vec<String> {
    let mut args = vec![
        "test".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        target_dir.to_owned(),
        "-p".to_owned(),
        package.to_owned(),
    ];
    if let Some(features) = features {
        args.extend(["--features".to_owned(), features.to_owned()]);
    }
    args.extend([
        "--test".to_owned(),
        target.to_owned(),
        "--jobs".to_owned(),
        jobs.max(1).to_string(),
        "--".to_owned(),
        "--nocapture".to_owned(),
    ]);
    args
}
