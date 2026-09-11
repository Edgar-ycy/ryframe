use std::{fs, path::Path, process::Command};

use crate::Result;

use super::{
    metadata::corepack_executable,
    model::{
        BaselineContract, DevexRunOptions, DevexSuite, PairedArm, SuiteDefinition, SuiteRequirement,
    },
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
    if contract == Some(BaselineContract::LegacyStableReadinessB0V1)
        && options.suite == DevexSuite::FrontendFast
    {
        verify_frontend_fast_contract(frontend_root, options, arm)?;
    }
    Ok(())
}

fn verify_frontend_fast_contract(
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
            let output = Command::new(corepack_executable())
                .args(["pnpm", "check", "--plan"])
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
