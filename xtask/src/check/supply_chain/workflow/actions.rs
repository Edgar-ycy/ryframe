use std::path::Path;

use regex::Regex;

use super::{RunBlock, display_untrusted};
use super::{load_workflow, workflow_jobs};
use crate::check::supply_chain::{policy::Policy, yaml::YamlValue};

#[derive(Default)]
pub(super) struct ActionScan {
    pub(super) errors: Vec<String>,
    pub(super) installed_tools: Vec<(String, String)>,
    pub(super) references: Vec<String>,
    pub(super) run_blocks: Vec<RunBlock>,
}

#[allow(dead_code)]
pub(crate) fn validate_action_uses(path: &Path, source: &str, policy: &Policy) -> Vec<String> {
    let (document, errors) = load_workflow(path, source);
    let Some(document) = document else {
        return errors;
    };
    let (jobs, job_errors) = workflow_jobs(path, &document);
    let scan = scan(path, &jobs, policy);
    let mut result = errors;
    result.extend(job_errors);
    result.extend(scan.errors);
    result
}

pub(super) fn scan(path: &Path, jobs: &[(String, YamlValue)], policy: &Policy) -> ActionScan {
    let mut result = ActionScan::default();
    for (job_name, job) in jobs {
        let job_location = format!("jobs.{job_name}");
        if let Some(reference) = job.get("uses") {
            result
                .errors
                .extend(validate_reference(path, &job_location, reference, true));
            if let Some(reference) = reference.as_str() {
                result.references.push(reference.to_owned());
            }
        }
        let Some(steps) = job.get("steps") else {
            continue;
        };
        let Some(steps) = steps.as_sequence() else {
            result.errors.push(format!(
                "{}: {job_location}.steps 必须是数组",
                path.display()
            ));
            continue;
        };
        for (index, step) in steps.iter().enumerate() {
            let location = format!("{job_location}.steps[{index}]");
            if step.as_mapping().is_none() {
                result
                    .errors
                    .push(format!("{}: {location} 必须是对象", path.display()));
                continue;
            }
            if let Some(run) = step.get("run") {
                if let Some(source) = run.as_str() {
                    result.run_blocks.push(RunBlock {
                        path: path.to_path_buf(),
                        location: location.clone(),
                        source: source.to_owned(),
                    });
                } else {
                    result.errors.push(format!(
                        "{}: {location}.run 必须是静态字符串",
                        path.display()
                    ));
                }
            }
            let Some(reference) = step.get("uses") else {
                continue;
            };
            result
                .errors
                .extend(validate_reference(path, &location, reference, false));
            let Some(reference_text) = reference.as_str() else {
                continue;
            };
            result.references.push(reference_text.to_owned());
            if reference_text
                .to_ascii_lowercase()
                .starts_with("taiki-e/install-action@")
            {
                let (errors, installed) = validate_install_action(path, &location, step, policy);
                result.errors.extend(errors);
                result.installed_tools.extend(installed);
            }
        }
    }
    result
}

fn validate_reference(
    path: &Path,
    location: &str,
    reference: &YamlValue,
    reusable_workflow: bool,
) -> Vec<String> {
    let Some(reference) = reference.as_str() else {
        return vec![format!(
            "{}: {location}.uses 必须是静态字符串",
            path.display()
        )];
    };
    let valid = if reusable_workflow {
        if reference.starts_with("./") {
            local_reference(reference, LOCAL_REUSABLE_WORKFLOW_REF)
        } else {
            re(REUSABLE_WORKFLOW_REF).is_match(reference)
        }
    } else if reference.starts_with("./") {
        local_reference(reference, LOCAL_ACTION_REF)
    } else if reference.starts_with("docker://") {
        re(CONTAINER_ACTION_REF).is_match(reference)
    } else {
        re(ACTION_REF).is_match(reference)
    };
    if valid {
        return Vec::new();
    }
    let kind = if reusable_workflow {
        "可复用工作流"
    } else {
        "action"
    };
    vec![format!(
        "{}: {location} 的 {kind} 未固定到提交 SHA 或镜像 digest：{}",
        path.display(),
        display_untrusted(reference)
    )]
}

fn validate_install_action(
    path: &Path,
    location: &str,
    step: &YamlValue,
    policy: &Policy,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut errors = Vec::new();
    let mut installed = Vec::new();
    let Some(configuration) = step.get("with") else {
        return (
            vec![format!(
                "{}: {location} 的 install-action.with 必须是对象",
                path.display()
            )],
            installed,
        );
    };
    if configuration.as_mapping().is_none() {
        return (
            vec![format!(
                "{}: {location} 的 install-action.with 必须是对象",
                path.display()
            )],
            installed,
        );
    }
    if configuration.get("fallback").and_then(YamlValue::as_str) != Some("none") {
        errors.push(format!(
            "{}: {location} 的 install-action 必须禁用 fallback",
            path.display()
        ));
    }
    let Some(raw_tools) = configuration
        .get("tool")
        .and_then(YamlValue::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        errors.push(format!(
            "{}: {location} 的 install-action.tool 必须是非空字符串",
            path.display()
        ));
        return (errors, installed);
    };

    let mut seen = std::collections::BTreeSet::new();
    for specification in raw_tools.split(',') {
        let specification = specification.trim();
        let Some(capture) = re(INSTALL_TOOL_REF).captures(specification) else {
            errors.push(format!(
                "{}: {location} 的 install-action 工具未固定到完整版本：{}",
                path.display(),
                display_untrusted(specification)
            ));
            continue;
        };
        let name = capture.get(1).map(|value| value.as_str()).unwrap_or("");
        let version = capture.get(2).map(|value| value.as_str()).unwrap_or("");
        if !seen.insert(name.to_owned()) {
            errors.push(format!(
                "{}: {location} 的 install-action 重复声明工具 {name}",
                path.display()
            ));
            continue;
        }
        let Some(expected) = policy.tools.get(name) else {
            errors.push(format!(
                "{}: {location} 的工具 {name} 未在供应链策略声明",
                path.display()
            ));
            continue;
        };
        if version != expected {
            errors.push(format!(
                "工具 {name} 版本漂移：期望 {expected}，实际 {version}"
            ));
            continue;
        }
        installed.push((name.to_owned(), version.to_owned()));
    }
    (errors, installed)
}

fn local_reference(reference: &str, pattern: &str) -> bool {
    re(pattern).is_match(reference) && !reference.split('/').any(|component| component == "..")
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("固定工作流引用正则")
}

const ACTION_REF: &str = r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}$";
const REUSABLE_WORKFLOW_REF: &str =
    r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/\.github/workflows/[A-Za-z0-9_.-]+\.ya?ml@[0-9a-f]{40}$";
const LOCAL_ACTION_REF: &str = r"^\./[A-Za-z0-9_./-]+$";
const LOCAL_REUSABLE_WORKFLOW_REF: &str = r"^\./\.github/workflows/[A-Za-z0-9_.-]+\.ya?ml$";
const CONTAINER_ACTION_REF: &str = r"^docker://[A-Za-z0-9_.:/-]+@sha256:[0-9a-f]{64}$";
const INSTALL_TOOL_REF: &str = r"^([A-Za-z0-9_.-]+)@([0-9]+\.[0-9]+\.[0-9]+)$";
