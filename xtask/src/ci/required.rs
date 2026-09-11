use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Result,
    cli::{RequiredAction, RequiredEvent, RequiredJobResult, RequiredOptions},
};

use super::task_plan::{CiJob, ci_plan_for, plan_outputs, required_ci_jobs};

const FIXED_REQUIRED_JOBS: &[&str] = &["plan", "windows-smoke", "security-audit"];

pub(super) fn run(options: &RequiredOptions) -> Result<()> {
    let errors = validate_required_jobs(options)?;
    if !errors.is_empty() {
        return Err(errors.join("\n").into());
    }
    let action = match options.action {
        Some(action) => format!(", action={}", action.as_str()),
        None => String::new(),
    };
    println!(
        "Required 汇总校验通过（event={}{}）",
        options.event.as_str(),
        action
    );
    Ok(())
}

pub(crate) fn validate_required_jobs(options: &RequiredOptions) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    validate_names(
        "required job",
        required_job_names(),
        options.needs.keys().map(String::as_str).collect(),
        &mut errors,
    );
    let Some(plan) = options.needs.get("plan") else {
        return Ok(errors);
    };
    validate_names(
        "CI plan 输出",
        plan_output_names(),
        plan.outputs.keys().map(String::as_str).collect(),
        &mut errors,
    );
    let invalid = plan
        .outputs
        .iter()
        .filter_map(|(name, value)| (!matches!(value.as_str(), "true" | "false")).then_some(name))
        .cloned()
        .collect::<Vec<_>>();
    if !invalid.is_empty() {
        errors.push(format!(
            "CI plan 输出必须是 true/false：{}",
            invalid.join(", ")
        ));
    }
    if !errors.is_empty() {
        return Ok(errors);
    }
    validate_plan_contract(options, &plan.outputs, &mut errors)?;
    validate_results(options, &plan.outputs, &mut errors);
    Ok(errors)
}

fn validate_names(
    label: &str,
    expected: BTreeSet<&str>,
    actual: BTreeSet<&str>,
    errors: &mut Vec<String>,
) {
    let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
    let extra = actual.difference(&expected).copied().collect::<Vec<_>>();
    if !missing.is_empty() {
        errors.push(format!("缺少{label}：{}", missing.join(", ")));
    }
    if !extra.is_empty() {
        errors.push(format!("包含未知{label}：{}", extra.join(", ")));
    }
}

fn required_job_names() -> BTreeSet<&'static str> {
    FIXED_REQUIRED_JOBS
        .iter()
        .copied()
        .chain(required_ci_jobs().map(|(_, name)| name))
        .collect()
}

fn plan_output_names() -> BTreeSet<&'static str> {
    plan_outputs(&[])
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn validate_plan_contract(
    options: &RequiredOptions,
    actual: &BTreeMap<String, String>,
    errors: &mut Vec<String>,
) -> Result<()> {
    let fixed =
        options.event == RequiredEvent::Push || options.action == Some(RequiredAction::Edited);
    if !fixed {
        if !output_enabled(actual, CiJob::Preflight) {
            errors.push("每个 pull_request CI 计划都必须启用 preflight".into());
        }
        return Ok(());
    }
    let action = options.action.map(RequiredAction::as_str).unwrap_or("");
    let expected_jobs = ci_plan_for(options.event.as_str(), action, &Default::default(), false)?;
    let mismatch = plan_outputs(&expected_jobs)
        .into_iter()
        .any(|(name, enabled)| actual.get(name).map(String::as_str) != Some(bool_text(enabled)));
    if mismatch {
        let action = options
            .action
            .map(|value| format!(".{}", value.as_str()))
            .unwrap_or_default();
        errors.push(format!(
            "{}{action} 必须执行同一 CI 计划器选择的门禁",
            options.event.as_str()
        ));
    }
    Ok(())
}

fn validate_results(
    options: &RequiredOptions,
    outputs: &BTreeMap<String, String>,
    errors: &mut Vec<String>,
) {
    let edited = options.action == Some(RequiredAction::Edited);
    let mut expected = required_job_names()
        .into_iter()
        .map(|name| (name, RequiredJobResult::Skipped))
        .collect::<BTreeMap<_, _>>();
    expected.insert("plan", RequiredJobResult::Success);
    for (job, workflow_name) in required_ci_jobs() {
        let enabled = if job == CiJob::ResourceGate {
            output_enabled(outputs, CiJob::ResourceGate)
                || output_enabled(outputs, CiJob::ConsumerContract)
        } else {
            output_enabled(outputs, job)
        };
        expected.insert(
            workflow_name,
            if enabled {
                RequiredJobResult::Success
            } else {
                RequiredJobResult::Skipped
            },
        );
    }
    if !edited {
        expected.insert("security-audit", RequiredJobResult::Success);
        expected.insert("windows-smoke", RequiredJobResult::Success);
    }
    for (name, expected_result) in expected {
        let actual = options
            .needs
            .get(name)
            .expect("required 名称已在前置步骤完整核验")
            .result;
        if actual != expected_result {
            errors.push(format!(
                "{name} 期望 {}，实际 {}",
                expected_result.as_str(),
                actual.as_str()
            ));
        }
    }
}

fn output_enabled(outputs: &BTreeMap<String, String>, job: CiJob) -> bool {
    outputs.get(job.output_name()).map(String::as_str) == Some("true")
}

fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}
