use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    thread,
};

use serde::Serialize;

use crate::{
    Result,
    check::{
        backend_package_operation_args, ci_target_policy, ci_test_jobs_from,
        resource_workspace_compilation, verify_job_budget_from,
    },
    process::{run as run_process, run_owned, with_process_log},
    workspace::root_dir,
};

#[path = "resource_gate/model.rs"]
mod model;
#[path = "resource_gate/repository.rs"]
mod repository;

#[allow(unused_imports)]
pub(crate) use model::{
    ChangeStatus, ChangedFile, GateStep, OwnershipEntry, OwnershipManifest, ResourceChangeSet,
    ResourceDefinition, ResourceGateInput, analyze, delegates_generic_ci_path,
    full_fallback_reason_for_paths, plan_steps, should_run_for_paths,
};
#[allow(unused_imports)]
pub(crate) use repository::{
    parse_name_status, parse_ownership, parse_resource_definition, preferred_nonempty_ref,
};

const DECISION_FORMAT_VERSION: u16 = 1;
const DECISION_FILE_ENV: &str = "RYFRAME_RESOURCE_GATE_DECISION_FILE";
static NEXT_DECISION_ARTIFACT: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResourceGateMode {
    Targeted,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceGateDecision {
    pub(crate) format_version: u16,
    pub(crate) recognized: bool,
    pub(crate) mode: ResourceGateMode,
    pub(crate) fallback: Option<String>,
    pub(crate) steps: Vec<String>,
}

pub(crate) fn run(frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let recognized =
        targeted_activation_from(env::var("RYFRAME_RESOURCE_GATE_TARGETED").ok().as_deref());
    let loaded = match repository::load(&root) {
        Ok(loaded) => loaded,
        Err(reason) => {
            let change_set = ResourceChangeSet {
                ambiguous_reason: Some(reason),
                ..ResourceChangeSet::default()
            };
            write_decision_from_environment(&decision_for(&change_set, recognized))?;
            print_change_set(&change_set)?;
            return execute_plan(&root, frontend_dir, None, &change_set);
        }
    };
    let change_set = enforce_targeted_activation(analyze(&loaded.input), recognized);
    write_decision_from_environment(&decision_for(&change_set, recognized))?;
    println!("resource gate Git 范围：{}..{}", loaded.base, loaded.head);
    print_change_set(&change_set)?;
    execute_plan(&root, frontend_dir, Some(&loaded.base), &change_set)
}

const TARGETED_ACTIVATION: &str = "replay-verified-v1";

pub(crate) fn targeted_activation_from(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim() == TARGETED_ACTIVATION)
}

pub(crate) fn enforce_targeted_activation(
    mut change_set: ResourceChangeSet,
    targeted_enabled: bool,
) -> ResourceChangeSet {
    if !targeted_enabled && change_set.ambiguous_reason.is_none() {
        change_set.ambiguous_reason = Some(
            "targeted resource gate 尚未取得 replay 零分歧激活证据；required job 以完整门禁执行"
                .to_owned(),
        );
    }
    change_set
}

pub(crate) fn decision_for(
    change_set: &ResourceChangeSet,
    recognized: bool,
) -> ResourceGateDecision {
    ResourceGateDecision {
        format_version: DECISION_FORMAT_VERSION,
        recognized,
        mode: if change_set.ambiguous_reason.is_none() {
            ResourceGateMode::Targeted
        } else {
            ResourceGateMode::Full
        },
        fallback: change_set.ambiguous_reason.clone(),
        steps: plan_steps(change_set)
            .iter()
            .map(decision_step_label)
            .collect(),
    }
}

fn decision_step_label(step: &GateStep) -> String {
    match step {
        GateStep::FullRustGate => "full-rust-gate".to_owned(),
        GateStep::FullIntegration => "full-integration".to_owned(),
        GateStep::FullConsumerContract => "full-consumer-contract".to_owned(),
        GateStep::ResourceDrift => "resource-drift".to_owned(),
        GateStep::ResourceWorkspace => "resource-workspace".to_owned(),
        GateStep::AffectedClippy(packages) => package_step("affected-clippy", packages),
        GateStep::AffectedTest(packages) => package_step("affected-test", packages),
        GateStep::PermissionContract => "permission-contract".to_owned(),
        GateStep::MigrationContract => "migration-contract".to_owned(),
        GateStep::OpenApiAndFrontendConsumer => "openapi-and-frontend-consumer".to_owned(),
    }
}

fn package_step(label: &str, packages: &std::collections::BTreeSet<String>) -> String {
    format!(
        "{label}:{}",
        packages.iter().cloned().collect::<Vec<_>>().join(",")
    )
}

fn write_decision_from_environment(decision: &ResourceGateDecision) -> Result<()> {
    let Some(path) = env::var_os(DECISION_FILE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        return Ok(());
    };
    if !path.is_absolute() {
        return Err(format!("{DECISION_FILE_ENV} 必须是绝对路径").into());
    }
    write_decision_artifact(&path, decision)
}

pub(crate) fn write_decision_artifact(path: &Path, decision: &ResourceGateDecision) -> Result<()> {
    let parent = path
        .parent()
        .ok_or("resource gate decision 路径缺少父目录")?;
    if !parent.is_dir() {
        return Err(format!("resource gate decision 父目录不存在：{}", parent.display()).into());
    }
    if path.exists() {
        return Err(format!(
            "resource gate decision 已存在，拒绝覆盖：{}",
            path.display()
        )
        .into());
    }
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("resource gate decision 文件名不是有效 UTF-8")?;
    let sequence = NEXT_DECISION_ARTIFACT.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{filename}.{}-{sequence}.tmp", process::id()));
    let mut body = serde_json::to_vec_pretty(decision)?;
    body.push(b'\n');
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        output.write_all(&body)?;
        output.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn print_change_set(change_set: &ResourceChangeSet) -> Result<()> {
    println!(
        "ResourceChangeSet:\n{}",
        serde_json::to_string_pretty(change_set)?
    );
    if let Some(reason) = &change_set.ambiguous_reason {
        println!("resource gate 自动回退完整 Rust、集成、资源与消费者门禁：{reason}");
    } else {
        println!(
            "resource gate 定向范围：资源={}，后端输出={}，前端输出={}，crate={}",
            change_set.impacted_resources.len(),
            change_set.owned_backend_paths.len(),
            change_set.owned_frontend_paths.len(),
            change_set.affected_crates.len()
        );
    }
    Ok(())
}

fn execute_plan(
    root: &Path,
    frontend_dir: &Path,
    base: Option<&str>,
    change_set: &ResourceChangeSet,
) -> Result<()> {
    let available = std::thread::available_parallelism().map_or(4, usize::from);
    let budget =
        verify_job_budget_from(env::var("RYFRAME_VERIFY_JOBS").ok().as_deref(), available)?;
    let configured_test_jobs = env::var("RYFRAME_CI_TEST_JOBS").ok();
    let test_jobs = targeted_test_jobs_from(
        configured_test_jobs.as_deref(),
        cfg!(windows),
        budget.backend,
    )?;
    let targets = ci_target_policy()?;
    if change_set.ambiguous_reason.is_none() {
        return execute_targeted_plan(
            root,
            frontend_dir,
            base,
            change_set,
            budget,
            test_jobs,
            &targets,
        );
    }
    for step in plan_steps(change_set) {
        execute_step(root, frontend_dir, base, &step, budget, test_jobs, &targets)?;
    }
    Ok(())
}

pub(crate) fn targeted_test_jobs_from(
    configured: Option<&str>,
    windows: bool,
    backend_budget: usize,
) -> Result<usize> {
    Ok(ci_test_jobs_from(configured, windows, backend_budget)?.min(backend_budget))
}

#[allow(clippy::too_many_arguments)]
fn execute_targeted_plan(
    root: &Path,
    frontend_dir: &Path,
    base: Option<&str>,
    change_set: &ResourceChangeSet,
    budget: crate::check::VerifyJobBudget,
    test_jobs: usize,
    targets: &crate::check::VerifyTargetPolicy,
) -> Result<()> {
    let packages = change_set.affected_crates.clone();
    run_parallel_tasks(
        root,
        "resource-gate-workspace",
        || {
            execute_step(
                root,
                frontend_dir,
                base,
                &GateStep::ResourceDrift,
                budget,
                test_jobs,
                targets,
            )?;
            execute_step(
                root,
                frontend_dir,
                base,
                &GateStep::ResourceWorkspace,
                budget,
                test_jobs,
                targets,
            )
        },
        "resource-gate-contracts",
        || {
            for step in targeted_contract_steps(packages) {
                execute_step(root, frontend_dir, base, &step, budget, test_jobs, targets)?;
            }
            Ok(())
        },
    )
}

pub(crate) fn targeted_contract_steps(
    packages: std::collections::BTreeSet<String>,
) -> Vec<GateStep> {
    vec![
        GateStep::AffectedClippy(packages.clone()),
        GateStep::AffectedTest(packages),
        GateStep::PermissionContract,
        GateStep::MigrationContract,
        GateStep::OpenApiAndFrontendConsumer,
    ]
}

#[allow(clippy::too_many_arguments)]
fn execute_step(
    root: &Path,
    frontend_dir: &Path,
    base: Option<&str>,
    step: &GateStep,
    budget: crate::check::VerifyJobBudget,
    test_jobs: usize,
    targets: &crate::check::VerifyTargetPolicy,
) -> Result<()> {
    match step {
        GateStep::FullRustGate => super::rust_gate(frontend_dir)?,
        GateStep::FullIntegration => super::integration()?,
        GateStep::FullConsumerContract => super::consumer_contract(frontend_dir)?,
        GateStep::ResourceDrift => {
            run_owned(
                root,
                "cargo",
                &resource_check_args_for_target(frontend_dir, &targets.resource, budget.resource),
            )?;
        }
        GateStep::ResourceWorkspace => {
            resource_workspace_compilation(root, frontend_dir, &targets.resource, budget.resource)?
        }
        GateStep::AffectedClippy(packages) => run_owned(
            root,
            "cargo",
            &affected_package_args_for_target("clippy", packages, &targets.backend, budget.backend),
        )?,
        GateStep::AffectedTest(packages) => run_owned(
            root,
            "cargo",
            &affected_package_args_for_target("test", packages, &targets.backend, test_jobs),
        )?,
        GateStep::PermissionContract => {
            run_process(root, "python", &["scripts/check_permission_routes.py"])?;
        }
        GateStep::MigrationContract => {
            let args = super::preflight_migration_args(base);
            run_owned(root, "python", &args)?;
        }
        GateStep::OpenApiAndFrontendConsumer => super::consumer_contract(frontend_dir)?,
    }
    Ok(())
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

pub(crate) fn resource_check_args_for_target(
    frontend_dir: &Path,
    target_dir: &str,
    jobs: usize,
) -> Vec<String> {
    vec![
        "run".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        target_dir.to_owned(),
        "-p".to_owned(),
        "xtask".to_owned(),
        "--features".to_owned(),
        "resource".to_owned(),
        "--jobs".to_owned(),
        jobs.max(1).to_string(),
        "--".to_owned(),
        "resource".to_owned(),
        "--all".to_owned(),
        "--check".to_owned(),
        "--frontend-dir".to_owned(),
        frontend_dir.to_string_lossy().into_owned(),
    ]
}

pub(crate) fn affected_package_args_for_target(
    operation: &str,
    packages: &std::collections::BTreeSet<String>,
    target_dir: &str,
    jobs: usize,
) -> Vec<String> {
    let mut args = backend_package_operation_args(operation, packages, target_dir, jobs);
    let position = args
        .iter()
        .position(|argument| argument == "--jobs")
        .unwrap_or(args.len());
    args.insert(position, "--all-features".to_owned());
    args
}
