//! 定向资源门禁的编译、测试产物核对与测试进程执行。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    thread,
};

use crate::{
    Result,
    check::{BackendSnapshots, VerifyTargetPolicy},
    process::{command_output_with_env, run_owned, run_owned_with_env, with_process_log},
};

use super::{GateStep, affected_package_args_for_target};

pub(crate) fn targeted_test_jobs_from(
    configured: Option<&str>,
    backend_budget: usize,
) -> Result<usize> {
    let jobs = match configured {
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|jobs| (1..=64).contains(jobs))
            .ok_or("RYFRAME_RESOURCE_GATE_TEST_JOBS 必须是 1 到 64 的整数")?,
        None => backend_budget.max(1),
    };
    Ok(jobs.min(backend_budget.max(1)))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_targeted_compile_contracts(
    root: &Path,
    clippy: &GateStep,
    test: &GateStep,
    test_jobs: usize,
    targets: &VerifyTargetPolicy,
    snapshots: &BackendSnapshots,
) -> Result<()> {
    let GateStep::AffectedClippy(clippy_packages) = clippy else {
        return Err("resource gate 定向 Clippy 步骤类型无效".into());
    };
    let GateStep::AffectedTest(test_packages) = test else {
        return Err("resource gate 定向测试步骤类型无效".into());
    };
    if clippy_packages != test_packages {
        return Err("resource gate 的 Clippy/test crate 范围不一致".into());
    }
    run_owned(
        root,
        "cargo",
        &affected_package_args_for_target("clippy", clippy_packages, &targets.backend, test_jobs)?,
    )?;
    execute_targeted_test(root, test, test_jobs, targets, snapshots)
}

fn execute_targeted_test(
    root: &Path,
    step: &GateStep,
    test_jobs: usize,
    targets: &VerifyTargetPolicy,
    snapshots: &BackendSnapshots,
) -> Result<()> {
    let GateStep::AffectedTest(packages) = step else {
        return Err("resource gate 定向测试步骤类型无效".into());
    };
    let mut args = affected_package_args_for_target("test", packages, &targets.backend, test_jobs)?;
    let expected_tests = targeted_test_names_from_args(&args)?;
    args.extend([
        "--no-run".to_owned(),
        "--message-format=json-render-diagnostics".to_owned(),
    ]);
    let environment = snapshots.workspace_test_environment();
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let environment_refs = environment
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    let output = command_output_with_env(root, "cargo", &arg_refs, &environment_refs)?;
    let executables = targeted_test_executables_from_messages(&output, &expected_tests)?;
    run_targeted_test_executables(root, &executables, &environment)
}

pub(crate) fn targeted_test_names_from_args(args: &[String]) -> Result<BTreeSet<String>> {
    let tests = args
        .windows(2)
        .filter(|pair| pair[0] == "--test")
        .map(|pair| pair[1].clone())
        .collect::<BTreeSet<_>>();
    if tests.is_empty() {
        return Err("resource gate 定向测试命令缺少精确测试目标".into());
    }
    Ok(tests)
}

pub(crate) fn targeted_test_executables_from_messages(
    output: &str,
    expected: &BTreeSet<String>,
) -> Result<BTreeMap<String, PathBuf>> {
    let mut executables = BTreeMap::new();
    for message in output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
    {
        let is_artifact =
            message.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact");
        let name = message
            .pointer("/target/name")
            .and_then(serde_json::Value::as_str);
        let executable = message
            .get("executable")
            .and_then(serde_json::Value::as_str);
        let Some((name, executable)) = name.zip(executable) else {
            continue;
        };
        if !is_artifact || !expected.contains(name) {
            continue;
        }
        if executables
            .insert(name.to_owned(), PathBuf::from(executable))
            .is_some()
        {
            return Err(format!("Cargo 重复返回定向测试产物：{name}").into());
        }
    }
    let found = executables.keys().cloned().collect::<BTreeSet<_>>();
    let missing = expected.difference(&found).cloned().collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!("Cargo 输出缺少定向测试产物：{}", missing.join(", ")).into());
    }
    Ok(executables)
}

fn run_targeted_test_executables(
    root: &Path,
    executables: &BTreeMap<String, PathBuf>,
    environment: &[(&'static str, String)],
) -> Result<()> {
    let logs = root.join("target/verify/logs");
    let results = thread::scope(|scope| {
        executables
            .iter()
            .map(|(name, executable)| {
                let label = format!("resource-gate-test-{name}");
                let log = logs.join(format!("{label}.log"));
                scope.spawn(move || {
                    let executable = executable
                        .to_str()
                        .ok_or_else(|| "定向测试可执行文件路径不是有效 UTF-8".to_owned())?;
                    with_process_log(&label, &log, || {
                        run_owned_with_env(root, executable, &[], environment)
                    })
                    .map_err(|error| error.to_string())
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join())
            .collect::<Vec<_>>()
    });
    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push(error),
            Err(_) => failures.push("定向测试线程发生 panic".to_owned()),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("定向测试执行失败：{}", failures.join("；")).into())
    }
}
