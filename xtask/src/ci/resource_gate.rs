use std::{env, path::Path};

use crate::{
    Result,
    check::{
        backend_package_operation_args, ci_target_policy, ci_test_jobs_from,
        resource_workspace_compilation, verify_job_budget_from,
    },
    process::{run as run_process, run_owned},
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
    let test_jobs = ci_test_jobs_from(
        env::var("RYFRAME_CI_TEST_JOBS").ok().as_deref(),
        cfg!(windows),
        budget.total,
    )?;
    let targets = ci_target_policy()?;
    for step in plan_steps(change_set) {
        match step {
            GateStep::FullRustGate => super::rust_gate(frontend_dir)?,
            GateStep::FullIntegration => super::integration()?,
            GateStep::FullConsumerContract => super::consumer_contract(frontend_dir)?,
            GateStep::ResourceDrift => {
                run_owned(
                    root,
                    "cargo",
                    &resource_check_args_for_target(frontend_dir, &targets.resource),
                )?;
            }
            GateStep::ResourceWorkspace => resource_workspace_compilation(
                root,
                frontend_dir,
                &targets.resource,
                budget.resource,
            )?,
            GateStep::AffectedClippy(packages) => run_owned(
                root,
                "cargo",
                &affected_package_args_for_target(
                    "clippy",
                    &packages,
                    &targets.backend,
                    budget.backend,
                ),
            )?,
            GateStep::AffectedTest(packages) => run_owned(
                root,
                "cargo",
                &affected_package_args_for_target("test", &packages, &targets.backend, test_jobs),
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
    }
    Ok(())
}

pub(crate) fn resource_check_args_for_target(frontend_dir: &Path, target_dir: &str) -> Vec<String> {
    vec![
        "run".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        Path::new(target_dir)
            .join("driver")
            .to_string_lossy()
            .into_owned(),
        "-p".to_owned(),
        "xtask".to_owned(),
        "--features".to_owned(),
        "resource".to_owned(),
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
