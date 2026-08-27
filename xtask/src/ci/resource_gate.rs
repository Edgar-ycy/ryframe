use std::{env, path::Path};

use crate::{
    Result,
    check::{
        BACKEND_CI_TARGET_DIR, RESOURCE_CI_TARGET_DIR, backend_package_operation_args,
        ci_test_jobs_from, resource_workspace_compilation, verify_job_budget_from,
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
    ResourceDefinition, ResourceGateInput, analyze, plan_steps,
};
#[allow(unused_imports)]
pub(crate) use repository::{parse_name_status, parse_ownership, parse_resource_definition};

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
    let change_set = analyze(&loaded.input);
    println!("resource gate Git 范围：{}..{}", loaded.base, loaded.head);
    print_change_set(&change_set)?;
    execute_plan(&root, frontend_dir, Some(&loaded.base), &change_set)
}

fn print_change_set(change_set: &ResourceChangeSet) -> Result<()> {
    println!(
        "ResourceChangeSet:\n{}",
        serde_json::to_string_pretty(change_set)?
    );
    if let Some(reason) = &change_set.ambiguous_reason {
        println!("resource gate 自动回退完整 Rust/consumer 门禁：{reason}");
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
    for step in plan_steps(change_set) {
        match step {
            GateStep::FullRustGate => super::rust_gate(frontend_dir)?,
            GateStep::FullConsumerContract => super::consumer_contract(frontend_dir)?,
            GateStep::ResourceDrift => {
                run_owned(root, "cargo", &resource_check_args(frontend_dir))?;
            }
            GateStep::ResourceWorkspace => resource_workspace_compilation(
                root,
                frontend_dir,
                RESOURCE_CI_TARGET_DIR,
                budget.resource,
            )?,
            GateStep::AffectedClippy(packages) => run_owned(
                root,
                "cargo",
                &affected_package_args("clippy", &packages, budget.backend),
            )?,
            GateStep::AffectedTest(packages) => run_owned(
                root,
                "cargo",
                &affected_package_args("test", &packages, test_jobs),
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

pub(crate) fn resource_check_args(frontend_dir: &Path) -> Vec<String> {
    vec![
        "resource".to_owned(),
        "--all".to_owned(),
        "--check".to_owned(),
        "--frontend-dir".to_owned(),
        frontend_dir.to_string_lossy().into_owned(),
    ]
}

pub(crate) fn affected_package_args(
    operation: &str,
    packages: &std::collections::BTreeSet<String>,
    jobs: usize,
) -> Vec<String> {
    let mut args = backend_package_operation_args(operation, packages, BACKEND_CI_TARGET_DIR, jobs);
    let position = args
        .iter()
        .position(|argument| argument == "--jobs")
        .unwrap_or(args.len());
    args.insert(position, "--all-features".to_owned());
    args
}
