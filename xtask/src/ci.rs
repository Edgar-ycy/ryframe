use std::{
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
    cli::CiCommand,
    contract::verify_contract_source,
    process::{command_output, run_owned},
    workspace::root_dir,
};

#[path = "ci/databases.rs"]
pub(crate) mod databases;

#[path = "ci/frontend_source.rs"]
pub(crate) mod frontend_source;
#[path = "ci/integration_configuration.rs"]
pub(crate) mod integration_configuration;
#[path = "ci/integration_inputs.rs"]
mod integration_inputs;
#[path = "ci/required.rs"]
mod required;
#[path = "ci/resource_gate.rs"]
pub(crate) mod resource_gate;
#[path = "ci/security.rs"]
mod security;
#[path = "ci/task_plan.rs"]
mod task_plan;
#[path = "ci/tls.rs"]
pub(crate) mod tls;

#[allow(unused_imports)]
pub(crate) use required::validate_required_jobs;
#[allow(unused_imports)]
pub(crate) use security::source_command as security_source_command;
#[allow(unused_imports)]
pub(crate) use security::{
    deployment_commands, deployment_required_at, plan_at, plan_for_required, run_at,
};
#[allow(unused_imports)]
pub(crate) use task_plan::{
    CiJob, ci_execution_plan_for, ci_execution_plan_for_profile, ci_plan_for, plan_outputs,
    required_ci_jobs,
};

const FULL_CI_EVENTS: &[&str] = &["push", "schedule", "workflow_dispatch"];
const INTEGRATION_PACKAGES: &[&str] = &[
    "ryframe-adapters",
    "ryframe-db",
    "ryframe-tenant-db",
    "ryframe-generator",
];
const WINDOWS_RUST_GATE_PROFILE: &str = "windows-smoke";

pub(crate) fn run(command: CiCommand, frontend_dir: &Path) -> Result<()> {
    match command {
        CiCommand::Plan(base) => plan(base.as_deref()),
        CiCommand::ResourceGateReplay(options) => {
            resource_gate::replay::run(&options, frontend_dir)
        }
        CiCommand::FrontendSource(options) => {
            let task_plan = frontend_source::plan(&options)?;
            if options.plan {
                crate::check::render_task_plan(&task_plan);
                Ok(())
            } else {
                frontend_source::run(&options, &task_plan)
            }
        }
        command => execute_ci_command(&command, frontend_dir),
    }
}

fn execute_ci_command(command: &CiCommand, frontend_dir: &Path) -> Result<()> {
    let plan = ci_execution_plan_for(command)?;
    task_plan::execute_ci_job(command, &plan, frontend_dir, None)
}

fn plan(base: Option<&str>) -> Result<()> {
    let root = root_dir();
    let event = env::var("GITHUB_EVENT_NAME").unwrap_or_else(|_| "local".to_owned());
    let action = env::var("GITHUB_EVENT_ACTION").unwrap_or_default();
    let (paths, repository_range_valid) = ci_changed_paths(&root, &event, base)?;
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

fn ci_changed_paths(root: &Path, event: &str, base: Option<&str>) -> Result<(Vec<String>, bool)> {
    if FULL_CI_EVENTS.contains(&event) {
        // 定时、手动和主分支构建必须完整执行，具体文件列表不影响计划。
        return Ok((Vec::new(), configured_ci_range_valid(base)));
    }
    let head = env::var("GITHUB_SHA").ok();
    match (base, head.as_deref()) {
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

fn configured_ci_range_valid(base: Option<&str>) -> bool {
    let head = env::var("GITHUB_SHA").ok();
    matches!(
        (base, head.as_deref()),
        (Some(base), Some(head)) if valid_git_sha(base) && valid_git_sha(head)
    )
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

fn windows_smoke(frontend_dir: &Path, configured_test_jobs: Option<usize>) -> Result<()> {
    let root = root_dir();
    let jobs = ci_test_jobs_from(
        configured_test_jobs,
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

fn verify_frontend_checkout(frontend_dir: &Path, requested_ref: Option<&str>) -> Result<()> {
    let Some(requested_ref) = requested_ref.filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    let resolved = command_output(frontend_dir, "git", &["rev-parse", "--verify", "HEAD"])?;
    verify_frontend_checkout_ref(requested_ref, resolved.trim())?;
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

fn verify_formal_contract_source(
    frontend_dir: &Path,
    options: &crate::cli::ConsumerContractOptions,
) -> Result<()> {
    let Some(backend_head) = options.backend_head.as_deref() else {
        return Ok(());
    };
    let repository = options
        .backend_repository
        .as_deref()
        .ok_or("消费契约来源检查缺少 --backend-repository")?;
    let candidate = options
        .candidate_openapi
        .as_deref()
        .ok_or("消费契约来源检查缺少 --candidate-openapi")?;
    let root = root_dir();
    let commit = verify_contract_source(
        &root,
        backend_head,
        repository,
        &frontend_dir.join("openapi/source.json"),
        &frontend_dir.join("openapi/openapi.json"),
        candidate,
    )?;
    println!("{commit}");
    Ok(())
}

pub(crate) const INTEGRATION_TESTS: &[(&str, &str, Option<&str>)] = &[
    (
        "ryframe-db",
        "mysql_real_protocol",
        Some("repositories,migration"),
    ),
    ("ryframe-adapters", "redis_real_protocol", Some("redis-api")),
    (
        "ryframe-generator",
        "mysql_schema_import",
        Some("schema-import"),
    ),
    (
        "ryframe",
        "business_initialization_mysql",
        Some("bin-api,bin-migrate"),
    ),
];

fn integration() -> Result<()> {
    let root = root_dir();
    let configuration = integration_configuration::IntegrationConfiguration::load(&root)?;
    integration_inputs::require_service_inputs(&root)?;
    let targets = ci_target_policy()?;
    let jobs = configuration.test_jobs;
    for &(package, target, features) in INTEGRATION_TESTS {
        run_owned(
            &root,
            "cargo",
            &integration_test_args_for_target(package, target, features, &targets.backend, jobs),
        )?;
    }
    tls::run_from_configuration(&root, Path::new(&targets.backend), &configuration).map(|_| ())
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
        "--no-default-features".to_owned(),
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
