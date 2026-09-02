use std::{path::Path, process::Command};

use crate::Result;

use super::{
    metadata::corepack_executable,
    model::{DevexRunOptions, SuiteDefinition, SuiteRequirement},
};

pub(super) fn check(
    backend_root: &Path,
    frontend_root: &Path,
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
        SuiteRequirement::Frontend => require_frontend_tooling(frontend_root, options),
        SuiteRequirement::FrontendFiles => {
            require_frontend_manifest(frontend_root, options, "工作区")?;
            executable_available(backend_root, "cargo", &["--version"])
        }
    }
}

fn require_frontend_tooling(frontend_root: &Path, options: &DevexRunOptions) -> Result<()> {
    require_frontend_dependencies(frontend_root, options)?;
    executable_available(frontend_root, "node", &["--version"])?;
    executable_available(frontend_root, corepack_executable(), &["pnpm", "--version"])
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
