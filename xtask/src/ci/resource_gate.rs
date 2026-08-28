use std::{env, path::Path, thread};

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

pub(crate) fn run(frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let loaded = match repository::load(&root) {
        Ok(loaded) => loaded,
        Err(reason) => {
            let change_set = ResourceChangeSet {
                ambiguous_reason: Some(reason),
                ..ResourceChangeSet::default()
            };
            print_change_set(&change_set)?;
            return execute_plan(&root, frontend_dir, None, &change_set);
        }
    };
    let change_set = enforce_targeted_activation(
        analyze(&loaded.input),
        targeted_activation_from(env::var("RYFRAME_RESOURCE_GATE_TARGETED").ok().as_deref()),
    );
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
            change_set.relationship_closure.len(),
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
