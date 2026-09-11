use std::{collections::BTreeMap, fs, path::Path};

use super::{
    check::TaskExecutor,
    ci::{CiJob, ci_execution_plan_for, ci_plan_for, plan_outputs, validate_required_jobs},
    cli::{RequiredAction, RequiredEvent, RequiredJobResult, RequiredNeed, RequiredOptions},
};

fn options_for(
    event: RequiredEvent,
    action: Option<RequiredAction>,
    plan: &[CiJob],
) -> RequiredOptions {
    let outputs = plan_outputs(plan)
        .into_iter()
        .map(|(name, enabled)| (name.to_owned(), enabled.to_string()))
        .collect::<BTreeMap<_, _>>();
    let enabled = |job| plan.contains(&job);
    let mut needs = [
        ("plan", RequiredJobResult::Success),
        ("rust-gate", result_for(enabled(CiJob::RustGate))),
        (
            "resource-gate",
            result_for(enabled(CiJob::ResourceGate) || enabled(CiJob::ConsumerContract)),
        ),
        ("integration", result_for(enabled(CiJob::Integration))),
        ("windows-smoke", RequiredJobResult::Success),
        ("security-audit", RequiredJobResult::Success),
    ]
    .into_iter()
    .map(|(name, result)| {
        (
            name.to_owned(),
            RequiredNeed {
                result,
                outputs: BTreeMap::new(),
            },
        )
    })
    .collect::<BTreeMap<_, _>>();
    needs.get_mut("plan").unwrap().outputs = outputs;
    if action == Some(RequiredAction::Edited) {
        needs.get_mut("windows-smoke").unwrap().result = RequiredJobResult::Skipped;
        needs.get_mut("security-audit").unwrap().result = RequiredJobResult::Skipped;
    }
    RequiredOptions {
        event,
        action,
        needs,
    }
}

fn result_for(enabled: bool) -> RequiredJobResult {
    if enabled {
        RequiredJobResult::Success
    } else {
        RequiredJobResult::Skipped
    }
}

#[test]
fn required_accepts_full_and_edited_plans_from_the_shared_planner() {
    let full = [
        CiJob::Preflight,
        CiJob::RustGate,
        CiJob::ResourceGate,
        CiJob::Integration,
    ];
    assert!(
        validate_required_jobs(&options_for(RequiredEvent::Push, None, &full))
            .unwrap()
            .is_empty()
    );

    let edited = ci_plan_for("pull_request", "edited", &Default::default(), false).unwrap();
    let options = options_for(
        RequiredEvent::PullRequest,
        Some(RequiredAction::Edited),
        &edited,
    );
    assert_eq!(options.needs["plan"].outputs["preflight"], "true");
    assert!(validate_required_jobs(&options).unwrap().is_empty());
}

#[test]
fn required_rejects_mismatched_jobs_outputs_and_results() {
    let full = [
        CiJob::Preflight,
        CiJob::RustGate,
        CiJob::ResourceGate,
        CiJob::Integration,
    ];
    let base = options_for(RequiredEvent::Push, None, &full);

    let mut missing_job = base.clone();
    missing_job.needs.remove("rust-gate");
    assert!(contains_error(&missing_job, "缺少required job"));

    let mut extra_job = base.clone();
    extra_job.needs.insert(
        "unreviewed".into(),
        RequiredNeed {
            result: RequiredJobResult::Failure,
            outputs: BTreeMap::new(),
        },
    );
    assert!(contains_error(&extra_job, "包含未知required job"));

    let mut missing_output = base.clone();
    missing_output
        .needs
        .get_mut("plan")
        .unwrap()
        .outputs
        .remove("rust_gate");
    assert!(contains_error(&missing_output, "缺少CI plan 输出"));

    let mut invalid_output = base.clone();
    invalid_output
        .needs
        .get_mut("plan")
        .unwrap()
        .outputs
        .insert("rust_gate".into(), "yes".into());
    assert!(contains_error(&invalid_output, "必须是 true/false"));

    let mut failed_job = base;
    failed_job.needs.get_mut("rust-gate").unwrap().result = RequiredJobResult::Failure;
    assert!(contains_error(
        &failed_job,
        "rust-gate 期望 success，实际 failure"
    ));
}

#[test]
fn fixed_and_dynamic_plans_preserve_shared_planner_invariants() {
    let edited = ci_plan_for("pull_request", "edited", &Default::default(), false).unwrap();
    let mut options = options_for(
        RequiredEvent::PullRequest,
        Some(RequiredAction::Edited),
        &edited,
    );
    options
        .needs
        .get_mut("plan")
        .unwrap()
        .outputs
        .insert("preflight".into(), "false".into());
    assert!(contains_error(&options, "同一 CI 计划器"));

    let full = [
        CiJob::Preflight,
        CiJob::RustGate,
        CiJob::ResourceGate,
        CiJob::Integration,
    ];
    let mut push = options_for(RequiredEvent::Push, None, &full);
    push.needs
        .get_mut("plan")
        .unwrap()
        .outputs
        .insert("integration".into(), "false".into());
    assert!(contains_error(&push, "同一 CI 计划器"));

    let mut dynamic = options_for(
        RequiredEvent::PullRequest,
        Some(RequiredAction::Synchronize),
        &[CiJob::Preflight],
    );
    assert!(validate_required_jobs(&dynamic).unwrap().is_empty());
    dynamic
        .needs
        .get_mut("plan")
        .unwrap()
        .outputs
        .insert("preflight".into(), "false".into());
    assert!(contains_error(&dynamic, "必须启用 preflight"));
}

#[test]
fn required_uses_exactly_one_registered_task_node() {
    let options = options_for(RequiredEvent::Push, None, &[]);
    let plan = ci_execution_plan_for(&super::cli::CiCommand::Required(options)).unwrap();
    assert_eq!(plan.tasks.len(), 1);
    assert_eq!(plan.tasks[0].id, "ci.required-jobs");
    assert_eq!(plan.tasks[0].executor, TaskExecutor::CiRequiredJobs);
    assert!(plan.tasks[0].dependencies.is_empty());
}

#[test]
fn workflow_calls_xtask_required_with_the_complete_needs_document() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let workflow = fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    let required = workflow.split("\n  required:\n").nth(1).unwrap();
    assert!(required.contains("cargo xtask check ci required"));
    assert!(required.contains("${{ toJSON(needs) }}"));
    assert!(required.contains("--action \"$EVENT_ACTION\""));
    assert!(required.contains("dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30"));
    assert!(!required.contains("check_required_jobs.py"));
    assert!(!root.join("scripts/check_required_jobs.py").exists());
    assert!(
        !root
            .join("scripts/tests/test_check_required_jobs.py")
            .exists()
    );
}

fn contains_error(options: &RequiredOptions, expected: &str) -> bool {
    validate_required_jobs(options)
        .unwrap()
        .iter()
        .any(|error| error.contains(expected))
}
