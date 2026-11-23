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
    let mut outputs = plan_outputs(plan)
        .into_iter()
        .map(|(name, enabled)| (name.to_owned(), enabled.to_string()))
        .collect::<BTreeMap<_, _>>();
    let enabled = |job| plan.contains(&job);
    if enabled(CiJob::Preflight) {
        outputs.extend([
            (
                "preflight_frontend_repository".into(),
                "owner/frontend".into(),
            ),
            ("preflight_frontend_commit".into(), "1".repeat(40)),
        ]);
    }
    let mut needs = [
        ("plan", RequiredJobResult::Success),
        ("preflight-python", result_for(enabled(CiJob::Preflight))),
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
fn required_accepts_push_standard_and_edited_plans_from_the_shared_planner() {
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

    let pull_request = options_for(
        RequiredEvent::PullRequest,
        Some(RequiredAction::Synchronize),
        &[CiJob::Preflight],
    );
    assert_eq!(
        pull_request.needs["security-audit"].result,
        RequiredJobResult::Success
    );
    assert!(validate_required_jobs(&pull_request).unwrap().is_empty());

    let edited = ci_plan_for("pull_request", "edited", &Default::default(), false).unwrap();
    let options = options_for(
        RequiredEvent::PullRequest,
        Some(RequiredAction::Edited),
        &edited,
    );
    assert_eq!(options.needs["plan"].outputs["preflight"], "true");
    assert_eq!(
        options.needs["security-audit"].result,
        RequiredJobResult::Success
    );
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
fn required_rejects_failed_or_skipped_python_and_invalid_preflight_source() {
    let jobs = ci_plan_for("push", "", &Default::default(), false).unwrap();
    let original = options_for(RequiredEvent::Push, None, &jobs);
    assert!(validate_required_jobs(&original).unwrap().is_empty());

    for result in [
        RequiredJobResult::Failure,
        RequiredJobResult::Cancelled,
        RequiredJobResult::Skipped,
    ] {
        let mut failed = original.clone();
        failed.needs.get_mut("preflight-python").unwrap().result = result;
        assert!(contains_error(&failed, "preflight-python 期望 success"));
    }
    let mut missing = original.clone();
    missing.needs.remove("preflight-python");
    assert!(contains_error(
        &missing,
        "缺少required job：preflight-python"
    ));

    for (key, wrong) in [
        ("preflight_frontend_repository", Some("unknown")),
        ("preflight_frontend_commit", Some("main")),
        ("preflight_frontend_commit", None),
    ] {
        let mut invalid = original.clone();
        let outputs = &mut invalid.needs.get_mut("plan").unwrap().outputs;
        if let Some(value) = wrong {
            outputs.insert(key.into(), value.into());
        } else {
            outputs.remove(key);
        }
        assert!(contains_error(&invalid, "预检前端仓库与精确检出 SHA"));
    }
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
    use crate::check::supply_chain::yaml;
    use serde_json::json;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let workflow =
        yaml::parse(&fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap()).unwrap();
    let required = workflow.get("jobs").unwrap().get("required").unwrap();
    assert_eq!(
        required.get("if").unwrap().as_str(),
        Some("${{ always() }}")
    );
    assert!(required.get("env").is_none());
    let expected = options_for(RequiredEvent::Push, None, &[]);
    let jobs = required
        .get("needs")
        .unwrap()
        .as_sequence()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(jobs, expected.needs.keys().map(String::as_str).collect());
    let steps = required.get("steps").unwrap().as_sequence().unwrap();
    let invocation = steps
        .iter()
        .position(|step| step.get("shell").is_some())
        .unwrap();
    assert!(steps[..invocation].iter().any(|step| {
        step.get("run")
            .and_then(|value| value.as_str())
            .is_some_and(|value| {
                value.lines().eq([
                    "cargo build --locked --target-dir target/xtask-run -p xtask --bin xtask",
                    r#"printf '%s\n' "$GITHUB_WORKSPACE/target/xtask-run/debug" >> "$GITHUB_PATH""#,
                ])
            })
    }));
    let step = &steps[invocation];
    assert!(step.get("env").is_none());
    assert_eq!(
        step.get("shell").unwrap().as_str(),
        Some(r#"xtask check ci required --input "{0}""#)
    );
    let needs = expected
        .needs
        .iter()
        .map(|(name, need)| {
            (
                name.clone(),
                json!({"result":need.result.as_str(),"outputs":need.outputs}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let body = step
        .get("run")
        .unwrap()
        .as_str()
        .unwrap()
        .replace("${{ toJSON(github.event_name) }}", "\"push\"")
        .replace("${{ toJSON(github.event.action) }}", "null")
        .replace(
            "${{ toJSON(needs) }}",
            &serde_json::to_string(&needs).unwrap(),
        );
    let actual: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(actual, json!({"event":"push","action":null,"needs":needs}));
}
fn contains_error(options: &RequiredOptions, expected: &str) -> bool {
    validate_required_jobs(options)
        .unwrap()
        .iter()
        .any(|error| error.contains(expected))
}
