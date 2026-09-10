use std::path::{Path, PathBuf};

use crate::{
    Result,
    cli::{BuildOptions, BuildProfile},
    process::{run_owned, run_pnpm},
    workspace::root_dir,
};

const BACKEND_INPUTS: &[&str] = &[
    "Cargo 工作区清单、锁文件与所选包依赖图",
    "当前 Rust 工具链、目标平台与有效 Cargo 构建环境",
];
const FRONTEND_INPUTS: &[&str] = &[
    "前端 package.json、pnpm 锁文件及 build 任务依赖的源码与配置",
    "当前 Node、Corepack 环境与显式真实构建参数",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildExecutor {
    Cargo,
    Pnpm,
}

impl BuildExecutor {
    fn label(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Pnpm => "corepack pnpm",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildTask {
    pub(crate) id: &'static str,
    pub(crate) dependencies: &'static [&'static str],
    pub(crate) roles: &'static [&'static str],
    pub(crate) working_directory: PathBuf,
    pub(crate) executor: BuildExecutor,
    pub(crate) arguments: Vec<String>,
    pub(crate) inputs: &'static [&'static str],
    pub(crate) compilation: &'static str,
    pub(crate) allowed_writes: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildPlan {
    pub(crate) profile: BuildProfile,
    pub(crate) real: bool,
    pub(crate) tasks: Vec<BuildTask>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildSpec {
    profile: BuildProfile,
    real: bool,
    backend_root: PathBuf,
    frontend_root: PathBuf,
}

impl BuildSpec {
    fn new(options: BuildOptions, backend_root: &Path, frontend_root: &Path) -> Self {
        Self {
            profile: options.profile,
            real: options.real,
            backend_root: backend_root.to_path_buf(),
            frontend_root: frontend_root.to_path_buf(),
        }
    }

    fn into_plan(self) -> BuildPlan {
        let profile = self.profile;
        let real = self.real;
        BuildPlan {
            profile,
            real,
            tasks: vec![
                BuildTask {
                    id: "backend-runtime",
                    dependencies: &[],
                    roles: &["API", "Worker"],
                    working_directory: self.backend_root,
                    executor: BuildExecutor::Cargo,
                    arguments: cargo_arguments(profile),
                    inputs: BACKEND_INPUTS,
                    compilation: backend_compilation(profile),
                    allowed_writes: "target/build",
                },
                BuildTask {
                    id: "frontend-production",
                    dependencies: &["backend-runtime"],
                    roles: &["前端生产产物"],
                    working_directory: self.frontend_root,
                    executor: BuildExecutor::Pnpm,
                    arguments: frontend_arguments(real),
                    inputs: FRONTEND_INPUTS,
                    compilation: if real {
                        "production；真实同源配置与构建收据"
                    } else {
                        "production；bundle 预算核验"
                    },
                    allowed_writes: "前端 dist、构建缓存与后端 target/corepack-bin Corepack shim",
                },
            ],
        }
    }
}

pub(crate) fn run(options: BuildOptions, frontend_dir: &Path) -> Result<()> {
    let render_only = options.plan;
    let plan = build_plan(options, &root_dir(), frontend_dir);
    if render_only {
        print!("{}", render_plan(&plan));
        return Ok(());
    }
    execute_plan(&plan)
}

pub(crate) fn build_plan(
    options: BuildOptions,
    backend_root: &Path,
    frontend_root: &Path,
) -> BuildPlan {
    BuildSpec::new(options, backend_root, frontend_root).into_plan()
}

fn execute_plan(plan: &BuildPlan) -> Result<()> {
    for task in &plan.tasks {
        match task.executor {
            BuildExecutor::Cargo => run_owned(
                &task.working_directory,
                task.executor.label(),
                &task.arguments,
            )?,
            BuildExecutor::Pnpm => {
                let arguments = task
                    .arguments
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                run_pnpm(&task.working_directory, &arguments)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn render_plan(plan: &BuildPlan) -> String {
    let mut output = format!(
        "构建计划：profile={}，real={}\n",
        profile_label(plan.profile),
        plan.real
    );
    for task in &plan.tasks {
        let dependencies = display_values(task.dependencies, "无");
        output.push_str(&format!(
            "任务 {}：角色={}；依赖={}\n",
            task.id,
            task.roles.join("、"),
            dependencies
        ));
        output.push_str(&format!(
            "  工作目录={}；调用={} {}\n",
            task.working_directory.display(),
            task.executor.label(),
            task.arguments.join(" ")
        ));
        output.push_str(&format!(
            "  编译覆盖={}；输入={}；允许写入={}\n",
            task.compilation,
            task.inputs.join("、"),
            task.allowed_writes
        ));
    }
    output
}

fn cargo_arguments(profile: BuildProfile) -> Vec<String> {
    let mut arguments = [
        "build",
        "--locked",
        "--target-dir",
        "target/build",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-api,bin-worker",
        "--bin",
        "ryframe",
        "--bin",
        "ryframe-worker",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if profile == BuildProfile::Release {
        arguments.push("--release".to_owned());
    }
    arguments
}

fn frontend_arguments(real: bool) -> Vec<String> {
    if real {
        vec!["build".to_owned(), "--real".to_owned()]
    } else {
        vec!["build".to_owned()]
    }
}

fn backend_compilation(profile: BuildProfile) -> &'static str {
    match profile {
        BuildProfile::Release => {
            "host target；release；ryframe；features=bin-api,bin-worker；jobs=继承 Cargo 有效配置"
        }
        BuildProfile::Dev => {
            "host target；dev；ryframe；features=bin-api,bin-worker；jobs=继承 Cargo 有效配置"
        }
    }
}

fn profile_label(profile: BuildProfile) -> &'static str {
    match profile {
        BuildProfile::Release => "release",
        BuildProfile::Dev => "dev",
    }
}

fn display_values(values: &[&str], empty: &str) -> String {
    if values.is_empty() {
        empty.to_owned()
    } else {
        values.join("、")
    }
}
