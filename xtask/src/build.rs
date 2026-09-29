use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    Result,
    cli::{BuildOptions, BuildProfile},
    devex,
    process::{command_output, run_pnpm},
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildArtifact {
    Executable(&'static str),
    FrontendDirectory,
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
    pub(crate) artifact: BuildArtifact,
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
                    id: "backend-api",
                    dependencies: &[],
                    roles: &["API"],
                    working_directory: self.backend_root.clone(),
                    executor: BuildExecutor::Cargo,
                    arguments: cargo_arguments(profile, "bin-api", "ryframe"),
                    inputs: BACKEND_INPUTS,
                    compilation: backend_compilation(profile, "bin-api"),
                    allowed_writes: "target/build",
                    artifact: BuildArtifact::Executable("ryframe"),
                },
                BuildTask {
                    id: "backend-worker",
                    dependencies: &["backend-api"],
                    roles: &["Worker"],
                    working_directory: self.backend_root,
                    executor: BuildExecutor::Cargo,
                    arguments: cargo_arguments(profile, "bin-worker", "ryframe-worker"),
                    inputs: BACKEND_INPUTS,
                    compilation: backend_compilation(profile, "bin-worker"),
                    allowed_writes: "target/build",
                    artifact: BuildArtifact::Executable("ryframe-worker"),
                },
                BuildTask {
                    id: "frontend-production",
                    dependencies: &["backend-api", "backend-worker"],
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
                    artifact: BuildArtifact::FrontendDirectory,
                },
            ],
        }
    }
}

pub(crate) fn run(options: BuildOptions, frontend_dir: &Path) -> Result<()> {
    let render_only = options.plan;
    let backend_root = root_dir();
    let plan = build_plan(options, &backend_root, frontend_dir);
    if render_only {
        print!("{}", render_plan(&plan));
        return Ok(());
    }
    let source = devex::source_fingerprints(&backend_root, frontend_dir)?;
    execute_plan(&plan)?;
    if devex::source_fingerprints(&backend_root, frontend_dir)? != source {
        return Err("构建期间前后端源码发生变化，拒绝把混合来源产物作为成功结果".into());
    }
    Ok(())
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
        let artifact = match task.executor {
            BuildExecutor::Cargo => {
                let arguments = task
                    .arguments
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let messages =
                    command_output(&task.working_directory, task.executor.label(), &arguments)?;
                let BuildArtifact::Executable(binary) = task.artifact else {
                    return Err(format!("Cargo 构建任务 {} 缺少可执行文件声明", task.id).into());
                };
                summarize_executable(binary, &messages)?
            }
            BuildExecutor::Pnpm => {
                let arguments = task
                    .arguments
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                run_pnpm(&task.working_directory, &arguments)?;
                let BuildArtifact::FrontendDirectory = task.artifact else {
                    return Err(format!("前端构建任务 {} 的产物声明无效", task.id).into());
                };
                summarize_frontend(&task.working_directory)?
            }
        };
        println!(
            "产物 {}：{}；{} 字节；sha256:{}",
            task.roles.join("、"),
            artifact.path.display(),
            artifact.bytes,
            artifact.sha256
        );
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArtifactSummary {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

fn summarize_executable(binary: &str, messages: &str) -> Result<ArtifactSummary> {
    let path = cargo_executable_from_messages(binary, messages)?;
    if !path.is_file() {
        return Err(format!("Cargo 报告的 {binary} 产物不存在：{}", path.display()).into());
    }
    summarize_file(path)
}

pub(crate) fn cargo_executable_from_messages(binary: &str, messages: &str) -> Result<PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|message| {
            message.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact")
                && message
                    .pointer("/target/name")
                    .and_then(serde_json::Value::as_str)
                    == Some(binary)
                && message
                    .pointer("/target/kind")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")))
        })
        .and_then(|message| {
            message
                .get("executable")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
        })
        .ok_or_else(|| format!("Cargo 输出缺少 {binary} 可执行文件").into())
}

fn summarize_file(path: PathBuf) -> Result<ArtifactSummary> {
    let bytes = fs::metadata(&path)?.len();
    let mut file = File::open(&path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(ArtifactSummary {
        path,
        bytes,
        sha256: hex_digest(digest.finalize()),
    })
}

fn summarize_frontend(frontend_root: &Path) -> Result<ArtifactSummary> {
    let dist = frontend_root.join("dist");
    if !dist.join(".vite").join("manifest.json").is_file() {
        return Err(format!("前端构建产物缺少 Vite manifest：{}", dist.display()).into());
    }
    let mut files = Vec::new();
    collect_files(&dist, &dist, &mut files)?;
    files.sort();
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    for path in files {
        let relative = path
            .strip_prefix(&dist)?
            .to_string_lossy()
            .replace('\\', "/");
        let size = fs::metadata(&path)?.len();
        bytes = bytes.checked_add(size).ok_or("前端产物总字节数溢出")?;
        digest.update((relative.len() as u64).to_be_bytes());
        digest.update(relative.as_bytes());
        digest.update(size.to_be_bytes());
        let summary = summarize_file(path)?;
        digest.update(summary.sha256.as_bytes());
    }
    Ok(ArtifactSummary {
        path: dist,
        bytes,
        sha256: hex_digest(digest.finalize()),
    })
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(format!(
                "前端产物不能包含链接：{}",
                entry.path().strip_prefix(root)?.display()
            )
            .into());
        }
        if file_type.is_dir() {
            collect_files(root, &entry.path(), output)?;
        } else if file_type.is_file() {
            output.push(entry.path());
        }
    }
    Ok(())
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    let mut value = String::with_capacity(bytes.as_ref().len() * 2);
    for byte in bytes.as_ref() {
        use std::fmt::Write;
        write!(&mut value, "{byte:02x}").expect("写入 String 不会失败");
    }
    value
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

fn cargo_arguments(profile: BuildProfile, feature: &str, binary: &str) -> Vec<String> {
    let mut arguments = [
        "build",
        "--locked",
        "--target-dir",
        "target/build",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        feature,
        "--bin",
        binary,
        "--message-format=json-render-diagnostics",
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

fn backend_compilation(profile: BuildProfile, feature: &str) -> &'static str {
    match (profile, feature) {
        (BuildProfile::Release, "bin-api") => {
            "host target；release；ryframe API；features=bin-api；jobs=继承 Cargo 有效配置"
        }
        (BuildProfile::Release, "bin-worker") => {
            "host target；release；ryframe Worker；features=bin-worker；jobs=继承 Cargo 有效配置"
        }
        (BuildProfile::Dev, "bin-api") => {
            "host target；dev；ryframe API；features=bin-api；jobs=继承 Cargo 有效配置"
        }
        (BuildProfile::Dev, "bin-worker") => {
            "host target；dev；ryframe Worker；features=bin-worker；jobs=继承 Cargo 有效配置"
        }
        _ => unreachable!("BuildSpec 只生成已登记的后端角色"),
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
