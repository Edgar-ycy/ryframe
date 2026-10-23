use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

use crate::{
    Result,
    check::{
        BackendSnapshotProfile, BackendSnapshots, ci_consumer_contract_against_committed_snapshot,
        ci_target_policy, preflight_trusted_ref, prepare_backend_snapshots,
        resource_workspace_compilation, targeted_resource_workspace_compilation,
        verify_backend_snapshots, verify_job_budget_from,
    },
    migration::run_check as run_migration_history,
    process::run_owned,
    workspace::root_dir,
};

#[path = "resource_gate/cargo_surface.rs"]
mod cargo_surface;
#[path = "resource_gate/execution_support.rs"]
mod execution_support;
#[path = "resource_gate/model.rs"]
mod model;
#[path = "resource_gate/replay/mod.rs"]
pub(crate) mod replay;
#[path = "resource_gate/repository.rs"]
mod repository;
#[path = "resource_gate/targeted_execution.rs"]
mod targeted_execution;

pub(crate) use cargo_surface::affected_package_args_for_target;
pub(crate) use execution_support::resource_check_args_for_target;
use execution_support::run_parallel_tasks;
#[allow(unused_imports)]
pub(crate) use model::{
    ChangeStatus, ChangedFile, GateStep, OwnershipEntry, OwnershipManifest, ResourceChangeSet,
    ResourceDefinition, ResourceGateInput, analyze, delegates_generic_ci_path,
    full_fallback_reason_for_paths, plan_steps, should_run_for_paths,
};
#[allow(unused_imports)]
pub(crate) use repository::{
    load as load_repository, parse_name_status, parse_ownership, parse_resource_definition,
};
use targeted_execution::execute_targeted_compile_contracts;
#[allow(unused_imports)]
pub(crate) use targeted_execution::{
    targeted_test_executables_from_messages, targeted_test_jobs_from, targeted_test_names_from_args,
};

const DECISION_FORMAT_VERSION: u16 = 1;
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

pub(crate) fn run(frontend_dir: &Path, options: &crate::cli::ResourceGateOptions) -> Result<()> {
    let root = root_dir();
    let recognized = options.targeted;
    let loaded = match repository::load(&root, options) {
        Ok(loaded) => loaded,
        Err(reason) => {
            let change_set = ResourceChangeSet {
                ambiguous_reason: Some(reason),
                ..ResourceChangeSet::default()
            };
            write_decision(options, &decision_for(&change_set, recognized))?;
            print_change_set(&change_set)?;
            return execute_plan(&root, frontend_dir, None, &change_set, options);
        }
    };
    let change_set = enforce_targeted_activation(analyze(&loaded.input), recognized);
    write_decision(options, &decision_for(&change_set, recognized))?;
    println!("resource gate Git 范围：{}..{}", loaded.base, loaded.head);
    print_change_set(&change_set)?;
    execute_plan(
        &root,
        frontend_dir,
        Some(&loaded.base),
        &change_set,
        options,
    )
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

fn write_decision(
    options: &crate::cli::ResourceGateOptions,
    decision: &ResourceGateDecision,
) -> Result<()> {
    if let Some(path) = &options.decision_file {
        if !path.is_absolute() {
            return Err("--decision-file 必须是绝对路径".into());
        }
        write_decision_artifact(path, decision)?;
    }
    Ok(())
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
    options: &crate::cli::ResourceGateOptions,
) -> Result<()> {
    let available = std::thread::available_parallelism().map_or(4, usize::from);
    let budget = verify_job_budget_from(
        options.jobs.map(|value| value.to_string()).as_deref(),
        available,
    )?;
    let configured_test_jobs = options.test_jobs.map(|value| value.to_string());
    let test_jobs = targeted_test_jobs_from(configured_test_jobs.as_deref(), budget.backend)?;
    let targets = ci_target_policy()?;
    let execution = GateExecution {
        budget,
        test_jobs,
        targets: &targets,
        options,
    };
    if change_set.ambiguous_reason.is_none() {
        return execute_targeted_plan(root, frontend_dir, base, change_set, execution);
    }
    for step in plan_steps(change_set) {
        execute_step(root, frontend_dir, base, &step, execution)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct GateExecution<'a> {
    budget: crate::check::VerifyJobBudget,
    test_jobs: usize,
    targets: &'a crate::check::VerifyTargetPolicy,
    options: &'a crate::cli::ResourceGateOptions,
}

fn execute_targeted_plan(
    root: &Path,
    frontend_dir: &Path,
    base: Option<&str>,
    change_set: &ResourceChangeSet,
    execution: GateExecution<'_>,
) -> Result<()> {
    let packages = change_set.affected_crates.clone();
    let profiles = [BackendSnapshotProfile::OpenApiContract]
        .into_iter()
        .collect();
    let snapshots = prepare_backend_snapshots(root, &profiles)?;
    run_parallel_tasks(
        execution.budget.total > 1,
        root,
        "resource-gate-workspace",
        || {
            execute_step(
                root,
                frontend_dir,
                base,
                &GateStep::ResourceDrift,
                execution,
            )?;
            targeted_resource_workspace_compilation(
                root,
                frontend_dir,
                &execution.targets.resource,
                execution.budget.resource,
            )
        },
        "resource-gate-contracts",
        || execute_targeted_contracts(root, frontend_dir, base, packages, &snapshots, execution),
    )?;
    verify_backend_snapshots(root, &snapshots)
}

fn execute_targeted_contracts(
    root: &Path,
    frontend_dir: &Path,
    base: Option<&str>,
    packages: std::collections::BTreeSet<String>,
    snapshots: &BackendSnapshots,
    execution: GateExecution<'_>,
) -> Result<()> {
    let mut steps = targeted_contract_steps(packages).into_iter();
    let clippy = steps
        .next()
        .ok_or("resource gate 缺少 affected Clippy 步骤")?;
    let test = steps
        .next()
        .ok_or("resource gate 缺少 affected test 步骤")?;
    run_parallel_tasks(
        execution.options.jobs != Some(1),
        root,
        "resource-gate-compile-contracts",
        || {
            execute_targeted_compile_contracts(
                root,
                &clippy,
                &test,
                execution.test_jobs,
                execution.targets,
                snapshots,
            )
        },
        "resource-gate-cross-contracts",
        || execute_targeted_cross_contracts(root, frontend_dir, base, steps, execution.options),
    )
}

fn execute_targeted_cross_contracts(
    root: &Path,
    frontend_dir: &Path,
    base: Option<&str>,
    steps: impl Iterator<Item = GateStep>,
    options: &crate::cli::ResourceGateOptions,
) -> Result<()> {
    for step in steps {
        match step {
            GateStep::PermissionContract => {
                let violations = crate::check::permission_routes::violations(root)?;
                if !violations.is_empty() {
                    return Err(format!(
                        "访问边界违规：\n{}",
                        violations
                            .iter()
                            .map(|item| format!("  - {item}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                    .into());
                }
            }
            GateStep::MigrationContract => {
                run_migration_contract(root, base)?;
            }
            GateStep::OpenApiAndFrontendConsumer => {
                super::verify_frontend_checkout(frontend_dir, options.frontend_ref.as_deref())?;
                ci_consumer_contract_against_committed_snapshot(root, frontend_dir)?;
            }
            _ => return Err("resource gate 交叉契约步骤包含未知编译动作".into()),
        }
    }
    Ok(())
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

fn execute_step(
    root: &Path,
    frontend_dir: &Path,
    base: Option<&str>,
    step: &GateStep,
    execution: GateExecution<'_>,
) -> Result<()> {
    match step {
        GateStep::FullRustGate => execute_nested(
            &crate::cli::CiCommand::RustGate(crate::cli::RustGateOptions {
                frontend_ref: execution.options.frontend_ref.clone(),
                test_jobs: Some(execution.test_jobs),
                ..Default::default()
            }),
            frontend_dir,
            execution.budget,
        )?,
        GateStep::FullIntegration => {
            super::execute_ci_command(&crate::cli::CiCommand::Integration, frontend_dir)?
        }
        GateStep::FullConsumerContract => execute_nested(
            &crate::cli::CiCommand::ConsumerContract(nested_consumer_contract_options(
                execution.options,
            )),
            frontend_dir,
            execution.budget,
        )?,
        GateStep::ResourceDrift => {
            run_owned(
                root,
                "cargo",
                &resource_check_args_for_target(frontend_dir, &execution.targets.resource),
            )?;
        }
        GateStep::ResourceWorkspace => resource_workspace_compilation(
            root,
            frontend_dir,
            &execution.targets.resource,
            execution.budget.resource,
        )?,
        GateStep::AffectedClippy(packages) => run_owned(
            root,
            "cargo",
            &affected_package_args_for_target(
                "clippy",
                packages,
                &execution.targets.backend,
                execution.budget.backend,
            )?,
        )?,
        GateStep::AffectedTest(packages) => run_owned(
            root,
            "cargo",
            &affected_package_args_for_target(
                "test",
                packages,
                &execution.targets.backend,
                execution.test_jobs,
            )?,
        )?,
        GateStep::PermissionContract => crate::check::permission_routes::run(root)?,
        GateStep::MigrationContract => {
            run_migration_contract(root, base)?;
        }
        GateStep::OpenApiAndFrontendConsumer => execute_nested(
            &crate::cli::CiCommand::ConsumerContract(nested_consumer_contract_options(
                execution.options,
            )),
            frontend_dir,
            execution.budget,
        )?,
    }
    Ok(())
}

fn execute_nested(
    command: &crate::cli::CiCommand,
    frontend: &Path,
    budget: crate::check::VerifyJobBudget,
) -> Result<()> {
    let plan = super::ci_execution_plan_for(command)?;
    super::task_plan::execute_ci_job(command, &plan, frontend, Some(budget))
}

fn nested_consumer_contract_options(
    options: &crate::cli::ResourceGateOptions,
) -> crate::cli::ConsumerContractOptions {
    crate::cli::ConsumerContractOptions {
        frontend_ref: options.frontend_ref.clone(),
        ..Default::default()
    }
}

fn run_migration_contract(root: &Path, base: Option<&str>) -> Result<()> {
    let trusted_ref = preflight_trusted_ref(base).unwrap_or_else(|| "HEAD".to_owned());
    run_migration_history(root, true, &trusted_ref)
}
