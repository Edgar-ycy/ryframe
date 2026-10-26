//! 工作流接线回归；供应链、Required 和任务规划规则继续由既有 Rust 测试负责。
use crate::check::supply_chain::yaml::{self, YamlValue};
use std::fs;

#[path = "ci_workflow_gates.rs"]
mod gates;

fn workflow(name: &str) -> YamlValue {
    yaml::parse(
        &fs::read_to_string(
            crate::workspace::root_dir()
                .join(".github/workflows")
                .join(name),
        )
        .unwrap(),
    )
    .unwrap()
}

fn job<'a>(workflow: &'a YamlValue, name: &str) -> &'a YamlValue {
    workflow.get("jobs").unwrap().get(name).unwrap()
}

fn steps(job: &YamlValue) -> &[YamlValue] {
    job.get("steps").unwrap().as_sequence().unwrap()
}

fn text<'a>(value: &'a YamlValue, key: &str) -> &'a str {
    value.get(key).unwrap().as_str().unwrap()
}

fn uses(step: &YamlValue, action: &str) -> bool {
    step.get("uses")
        .and_then(YamlValue::as_str)
        .is_some_and(|value| value.starts_with(action))
}

fn command<'a>(job: &'a YamlValue, prefix: &str) -> (usize, &'a YamlValue) {
    steps(job)
        .iter()
        .enumerate()
        .find(|(_, step)| {
            step.get("run")
                .and_then(YamlValue::as_str)
                .is_some_and(|run| {
                    run.lines().any(|line| {
                        line.split_whitespace()
                            .take(prefix.split_whitespace().count())
                            .eq(prefix.split_whitespace())
                    })
                })
        })
        .unwrap_or_else(|| panic!("缺少工作流执行步骤：{prefix}"))
}

// 仅提取现行绑定中的 GitHub 表达式数据；不解释 Shell，单双引号和空白不构成契约。
fn github_value(step: &YamlValue, binding: &str) -> String {
    let pattern = format!(r#"(?s){binding}[\s'"]*(\$\{{\{{.*?\}}\}})"#);
    let expression = regex::Regex::new(&pattern)
        .unwrap()
        .captures(text(step, "run"))
        .unwrap()[1]
        .to_owned();
    expression.split_whitespace().collect()
}

#[test]
fn commit_range_expressions_do_not_use_tag_before_as_a_commit_baseline() {
    let ci = workflow("ci.yml");
    let expected = "${{github.event.pull_request.base.sha||(github.ref_type=='branch'&&github.event.before)||''}}";
    for action in ["plan", "preflight"] {
        let (_, step) = command(job(&ci, "plan"), &format!("cargo xtask check ci {action}"));
        assert_eq!(github_value(step, r"--base-sha(?:\s+|=)"), expected);
    }
    let (_, resource) = command(
        job(&ci, "resource-gate"),
        "cargo xtask check ci resource-gate",
    );
    assert_eq!(github_value(resource, r"(?m)^\s*base\s*="), expected);
    for phase in ["source", "image"] {
        let (_, step) = command(
            job(&ci, "security-audit"),
            &format!("cargo xtask check ci security deployment {phase}"),
        );
        assert_eq!(
            text(step.get("env").unwrap(), "DEPLOYMENT_BASE_SHA")
                .split_whitespace()
                .collect::<String>(),
            expected
        );
    }
}

#[test]
fn frontend_selection_precedes_checkout_and_the_consumer_uses_exact_source_inputs() {
    for file in ["ci.yml", "extended-ci.yml"] {
        let workflow = workflow(file);
        for (_, job) in workflow.get("jobs").unwrap().as_mapping().unwrap() {
            for (selection, selector) in steps(job).iter().enumerate().filter(|(_, step)| {
                step.get("run")
                    .and_then(YamlValue::as_str)
                    .is_some_and(|run| {
                        run.split_whitespace().take(5).eq([
                            "cargo",
                            "xtask",
                            "check",
                            "ci",
                            "frontend-source",
                        ])
                    })
            }) {
                assert!(
                    steps(job)[..selection]
                        .iter()
                        .any(|step| uses(step, "dtolnay/rust-toolchain@"))
                );
                let id = text(selector, "id");
                assert!(steps(job)[selection + 1..].iter().any(|step| {
                    uses(step, "actions/checkout@")
                        && step
                            .get("with")
                            .and_then(|value| value.get("ref"))
                            .and_then(YamlValue::as_str)
                            .is_some_and(|value| value.contains(&format!("steps.{id}.outputs.ref")))
                }));
            }
        }
    }
    let ci = workflow("ci.yml");
    for name in ["plan", "rust-gate", "resource-gate", "windows-smoke"] {
        command(job(&ci, name), "cargo xtask check ci frontend-source");
    }
    let (_, consumer) = command(
        job(&ci, "resource-gate"),
        "cargo xtask check ci consumer-contract",
    );
    for (option, expected) in [
        (
            r"--frontend-ref(?:\s+|=)",
            "${{steps.contract-frontend-ref.outputs.ref}}",
        ),
        (
            r"--backend-head(?:\s+|=)",
            "${{github.event.pull_request.head.sha}}",
        ),
        (r"--backend-repository(?:\s+|=)", "${{github.repository}}"),
    ] {
        assert_eq!(github_value(consumer, option), expected);
    }
    assert!(
        !steps(job(&ci, "plan"))
            .iter()
            .any(|step| uses(step, "actions/setup-node@"))
    );
}
