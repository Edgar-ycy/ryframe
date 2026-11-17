use std::{fs, path::Path, process::Command};

use serde::Deserialize;

use crate::Result;

use super::{
    metadata::corepack_executable,
    model::{
        BaselineContract, DevexRunOptions, DevexSuite, PairedArm, SuiteDefinition,
        SuiteRequirement, is_common_frontend_fast,
    },
    support::frontend_fast_arguments,
};

pub(super) fn check(
    backend_root: &Path,
    frontend_root: &Path,
    runner_frontend_root: &Path,
    options: &DevexRunOptions,
    definition: SuiteDefinition,
) -> Result<()> {
    match definition.requirement {
        SuiteRequirement::Ready => executable_available(backend_root, "cargo", &["--version"]),
        SuiteRequirement::Executable(executable) => {
            if definition.requires_frontend {
                require_frontend_tooling(frontend_root, options)?;
            }
            executable_available(backend_root, executable, &["--version"])
        }
        SuiteRequirement::Frontend if options.suite.is_runtime() => {
            require_runtime_frontend_tooling(frontend_root, runner_frontend_root, options)
        }
        SuiteRequirement::Frontend => require_frontend_tooling(frontend_root, options),
        SuiteRequirement::FrontendFiles => {
            require_frontend_manifest(frontend_root, options, "工作区")?;
            executable_available(backend_root, "cargo", &["--version"])
        }
    }
}

pub(super) fn check_paired(
    backend_root: &Path,
    frontend_root: &Path,
    runner_frontend_root: &Path,
    options: &DevexRunOptions,
    definition: SuiteDefinition,
    contract: Option<BaselineContract>,
    arm: PairedArm,
) -> Result<()> {
    check(
        backend_root,
        frontend_root,
        runner_frontend_root,
        options,
        definition,
    )?;
    if options.suite == DevexSuite::FrontendFast {
        match contract {
            Some(BaselineContract::LegacyStableReadinessB0V1) => {
                verify_frontend_fast_contract(backend_root, frontend_root, options, arm)?;
            }
            Some(contract) if contract.uses_common_toolchain() => {
                verify_common_frontend_fast(
                    frontend_root,
                    runner_frontend_root,
                    definition,
                    contract,
                    arm,
                )?;
            }
            _ => {}
        }
    }
    if options.suite == DevexSuite::CargoDevSave
        && options.variant == "resource-manifest"
        && contract.is_some_and(BaselineContract::uses_common_toolchain)
    {
        verify_save_resource_frontend(backend_root, frontend_root, arm)?;
    }
    Ok(())
}

pub(super) fn verify_save_resource_frontend(
    backend: &Path,
    expected: &Path,
    arm: PairedArm,
) -> Result<()> {
    // 保存反馈触发的是子门禁，外层 paired 的前端目录不会传给它。
    let selected = match arm {
        PairedArm::Baseline => backend
            .parent()
            .ok_or("B0 后端工作树缺少用于定位前端的父目录")?
            .join("ryframe-vue3"),
        PairedArm::Candidate => ryframe_config::ToolSettings::load(backend)?.frontend_dir(backend),
    };
    let selected = selected
        .canonicalize()
        .map_err(|error| format!("{} 保存测量的资源子门禁前端无效：{error}", arm.as_str()))?;
    if selected != expected.canonicalize()? {
        return Err(format!(
            "{} 保存测量的资源子门禁前端与登记的前端工作树不同",
            arm.as_str()
        )
        .into());
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedFastPlan {
    arm: String,
    tasks: Vec<SharedFastTask>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedFastTask {
    id: String,
    covers: Vec<String>,
    node: Option<Vec<String>>,
    pnpm: Option<Vec<String>>,
}

fn verify_common_frontend_fast(
    frontend: &Path,
    runner: &Path,
    definition: SuiteDefinition,
    contract: BaselineContract,
    arm: PairedArm,
) -> Result<()> {
    let [step] = definition.steps else {
        return Err("共同 DevEx 前端快速检查必须使用一个共享入口".into());
    };
    if !is_common_frontend_fast(step) {
        return Err("共同 DevEx 前端快速检查缺少共享入口".into());
    }
    let runner_source = super::metadata::source_state(runner)?;
    if runner_source.commit.as_deref() != Some(BaselineContract::COMMON_FRONTEND_RUNNER_COMMIT)
        || runner_source.dirty != Some(false)
    {
        return Err("共同 DevEx 快速检查须使用已验收且干净的前端工具提交 66d9f747".into());
    }
    let output = Command::new(step.program)
        .args(frontend_fast_arguments(
            step,
            frontend,
            runner,
            Path::new("plan"),
        ))
        .arg("--plan")
        .current_dir(frontend)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "共同 DevEx 前端任务计划检查失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let actual = serde_json::from_slice::<SharedFastPlan>(&output.stdout)?;
    let workload = contract
        .workload_contract(DevexSuite::FrontendFast, "default")
        .ok_or("共同 DevEx 前端快速检查缺少逻辑任务合同")?;
    let (label, expected) = match arm {
        PairedArm::Baseline => ("b0", workload.baseline_primitives),
        PairedArm::Candidate => ("b1", workload.candidate_primitives),
    };
    let valid = actual.arm == label
        && actual.tasks.len() == expected.len()
        && actual.tasks.iter().zip(expected).all(|(actual, expected)| {
            actual.id == expected.id
                && actual.covers == expected.covers
                && match (&actual.node, &actual.pnpm) {
                    (Some(args), None) | (None, Some(args)) => !args.is_empty(),
                    _ => false,
                }
        });
    if !valid {
        return Err(format!("共同 DevEx {label} 计划与九项前端逻辑检查不一致").into());
    }
    if super::metadata::source_state(runner)? != runner_source {
        return Err("共同 DevEx 前端工具在计划检查期间发生变化".into());
    }
    Ok(())
}

fn verify_frontend_fast_contract(
    backend_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
    arm: PairedArm,
) -> Result<()> {
    let contract = BaselineContract::LegacyStableReadinessB0V1
        .workload_contract(options.suite, &options.variant)
        .ok_or("stable-readiness frontend-fast 缺少逻辑任务合同")?;
    match arm {
        PairedArm::Baseline => {
            let manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(frontend_root.join("package.json"))?)?;
            if manifest
                .pointer("/scripts/check:fast")
                .and_then(|value| value.as_str())
                != Some("node scripts/run-fast-checks.mjs")
                || !frontend_root.join("scripts/run-fast-checks.mjs").is_file()
            {
                return Err("stable-readiness B0 前端缺少已登记 check:fast 执行器".into());
            }
        }
        PairedArm::Candidate => {
            let mut command = Command::new(corepack_executable());
            command.arg("pnpm");
            if let Some(cache) = crate::process::pnpm_cache_argument(backend_root)? {
                command.arg(cache);
            }
            let output = command
                .args(["check", "--plan"])
                .current_dir(frontend_root)
                .output()?;
            if !output.status.success() {
                return Err(format!(
                    "候选前端无法只读解析统一快速任务图：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )
                .into());
            }
            let actual = parse_frontend_fast_plan(&String::from_utf8(output.stdout)?);
            let expected = contract
                .candidate_primitives
                .iter()
                .map(|primitive| primitive.id.clone())
                .collect::<Vec<_>>();
            if actual != expected {
                return Err(format!(
                    "候选前端快速任务集合与性能合同不一致：预期 {expected:?}，实际 {actual:?}"
                )
                .into());
            }
        }
    }
    Ok(())
}

pub(crate) fn parse_frontend_fast_plan(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            line.strip_prefix("    ")
                .and_then(|line| line.split_once(' '))
                .map(|(id, _)| id.to_owned())
        })
        .collect()
}

fn require_frontend_tooling(frontend_root: &Path, options: &DevexRunOptions) -> Result<()> {
    require_frontend_dependencies(frontend_root, options)?;
    executable_available(frontend_root, "node", &["--version"])?;
    executable_available(frontend_root, corepack_executable(), &["pnpm", "--version"])
}

pub(crate) fn require_runtime_frontend_tooling(
    product_frontend_root: &Path,
    runner_frontend_root: &Path,
    options: &DevexRunOptions,
) -> Result<()> {
    require_runtime_frontend_layout(product_frontend_root, runner_frontend_root, options)?;
    executable_available(runner_frontend_root, "node", &["--version"])?;
    executable_available(
        runner_frontend_root,
        corepack_executable(),
        &["pnpm", "--version"],
    )
}

pub(crate) fn require_runtime_frontend_layout(
    product_frontend_root: &Path,
    runner_frontend_root: &Path,
    options: &DevexRunOptions,
) -> Result<()> {
    require_frontend_manifest(product_frontend_root, options, "产品工作区")?;
    require_frontend_dependencies(runner_frontend_root, options)?;
    require_runtime_frontend_files(runner_frontend_root, options)
}

pub(crate) fn require_frontend_dependencies(
    frontend_root: &Path,
    options: &DevexRunOptions,
) -> Result<()> {
    require_frontend_manifest(frontend_root, options, "目录")?;
    if frontend_root.join("node_modules").is_dir() {
        return Ok(());
    }
    Err(format!(
        "suite `{}` 需要已安装的前端依赖；请先在 {} 运行 corepack pnpm install --frozen-lockfile",
        options.suite.as_str(),
        frontend_root.display()
    )
    .into())
}

pub(crate) fn require_runtime_frontend_files(
    frontend_root: &Path,
    options: &DevexRunOptions,
) -> Result<()> {
    const FILES: &[&str] = &[
        "scripts/browser-login-budget.mjs",
        "scripts/browser-login-budget-model.mjs",
        "scripts/browser-login-ledger.mjs",
    ];
    let missing = FILES
        .iter()
        .filter(|relative| !frontend_root.join(relative).is_file())
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "suite `{}` 缺少前端共享登录预算文件：{}",
            options.suite.as_str(),
            missing.join("、")
        )
        .into())
    }
}

fn require_frontend_manifest(
    frontend_root: &Path,
    options: &DevexRunOptions,
    kind: &str,
) -> Result<()> {
    if frontend_root.join("package.json").is_file() {
        Ok(())
    } else {
        Err(format!(
            "suite `{}` 需要前端{kind} {}",
            options.suite.as_str(),
            frontend_root.display()
        )
        .into())
    }
}

fn executable_available(root: &Path, executable: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(executable)
        .args(args)
        .current_dir(root)
        .output();
    match output {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "DevEx 前置检查失败：`{executable} {}` 返回 {}",
            args.join(" "),
            output.status
        )
        .into()),
        Err(error) => Err(format!("DevEx 前置检查找不到 `{executable}`：{error}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{PairedArm, fs, verify_save_resource_frontend};

    #[test]
    fn save_resource_preflight_tracks_each_actual_child_tool_root() {
        let scratch = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&scratch).unwrap();
        let root = tempfile::tempdir_in(scratch).unwrap();
        let backend = root.path().join("backend");
        let baseline = root.path().join("ryframe-vue3");
        let candidate = root.path().join("configured-frontend");
        fs::create_dir_all(backend.join(".local-tests")).unwrap();
        fs::create_dir(&baseline).unwrap();
        fs::create_dir(&candidate).unwrap();
        fs::write(
            backend.join(".local-tests/tools.toml"),
            "[workspace]\nfrontend_dir = '../configured-frontend'\n",
        )
        .unwrap();

        verify_save_resource_frontend(&backend, &baseline, PairedArm::Baseline).unwrap();
        verify_save_resource_frontend(&backend, &candidate, PairedArm::Candidate).unwrap();
        assert!(verify_save_resource_frontend(&backend, &candidate, PairedArm::Baseline).is_err());
        assert!(verify_save_resource_frontend(&backend, &baseline, PairedArm::Candidate).is_err());
    }
}
