use std::{fmt, fs, path::Path};

use crate::{
    Result,
    process::{command_version_output, pnpm_version_output, run as run_process},
    workspace::root_dir,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ToolVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl ToolVersion {
    pub(crate) fn parse(value: &str, source: &str) -> Result<Self> {
        let value = value.trim().trim_start_matches('v');
        let mut parts = value.split('.');
        let major = parse_version_component(parts.next(), source, value)?;
        let minor = parse_version_component(parts.next(), source, value)?;
        let patch = parse_version_component(parts.next(), source, value)?;
        if parts.next().is_some() {
            return Err(format!("{source} 不是有效的三段式版本号：{value}").into());
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for ToolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

struct FrontendToolchain {
    node_engine: String,
    pnpm_version: ToolVersion,
    preferred_node: ToolVersion,
}

pub(crate) fn run(frontend_dir: &Path) -> Result<()> {
    run_backend()?;
    check_frontend(frontend_dir)?;
    println!("RyFrame 开发环境检查通过。");
    Ok(())
}

pub(crate) fn run_backend() -> Result<()> {
    let root = root_dir();
    check_backend_at(&root)?;
    crate::check::python_environment::run(&root)?;
    println!("RyFrame 后端开发环境检查通过。");
    Ok(())
}

pub(crate) fn run_backend_at(root: &Path) -> Result<()> {
    check_backend_at(root)?;
    println!("RyFrame 后端开发环境检查通过。");
    Ok(())
}

fn check_backend_at(root: &Path) -> Result<()> {
    for executable in ["cargo", "rustc", "git"] {
        run_process(root, executable, &["--version"])?;
    }

    let path = root.join(".git");
    if !path.exists() {
        return Err(format!("缺少后端 Git 仓库：{}", path.display()).into());
    }
    Ok(())
}

fn check_frontend(frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    for (name, path) in [
        ("前端目录", frontend_dir.to_path_buf()),
        ("前端 Git 仓库", frontend_dir.join(".git")),
    ] {
        if !path.exists() {
            return Err(format!("缺少{name}：{}", path.display()).into());
        }
    }
    let toolchain = frontend_toolchain(frontend_dir)?;
    let node_version = command_version(&root, "node")?;
    if !node_engine_satisfies(node_version, &toolchain.node_engine)? {
        return Err(format!(
            "Node.js {node_version} 不满足前端 engines.node={}；请安装至少 {} 或另一受支持版本",
            toolchain.node_engine, toolchain.preferred_node
        )
        .into());
    }
    let pnpm_version = corepack_pnpm_version(frontend_dir)?;
    if pnpm_version != toolchain.pnpm_version {
        return Err(format!(
            "pnpm {pnpm_version} 与前端 packageManager=pnpm@{} 不一致；请通过 Corepack 使用固定版本",
            toolchain.pnpm_version
        )
        .into());
    }

    Ok(())
}

fn command_version(dir: &Path, executable: &str) -> Result<ToolVersion> {
    let output = command_version_output(dir, executable)?;
    ToolVersion::parse(&output, &format!("{executable} --version"))
}

fn corepack_pnpm_version(dir: &Path) -> Result<ToolVersion> {
    let output = pnpm_version_output(dir)?;
    ToolVersion::parse(&output, "项目级 pnpm --version")
}

fn frontend_toolchain(frontend_dir: &Path) -> Result<FrontendToolchain> {
    let package_path = frontend_dir.join("package.json");
    let package_source = fs::read_to_string(&package_path)
        .map_err(|error| format!("无法读取 {}：{error}", package_path.display()))?;
    let package: serde_json::Value = serde_json::from_str(&package_source)
        .map_err(|error| format!("{} 不是有效 JSON：{error}", package_path.display()))?;
    let node_engine = package
        .pointer("/engines/node")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("前端 package.json 缺少 engines.node")?
        .trim()
        .to_owned();
    let package_manager = package
        .get("packageManager")
        .and_then(serde_json::Value::as_str)
        .ok_or("前端 package.json 缺少 packageManager")?;
    let pnpm_version = package_manager
        .strip_prefix("pnpm@")
        .ok_or("前端 packageManager 必须使用 pnpm@<版本>")?;
    let preferred_node_path = frontend_dir.join(".node-version");
    let preferred_node = fs::read_to_string(&preferred_node_path)
        .map_err(|error| format!("无法读取 {}：{error}", preferred_node_path.display()))?;
    let preferred_node = ToolVersion::parse(&preferred_node, ".node-version")?;
    if !node_engine_satisfies(preferred_node, &node_engine)? {
        return Err(format!(
            ".node-version 的 {} 不满足 package.json engines.node={node_engine}",
            preferred_node
        )
        .into());
    }
    Ok(FrontendToolchain {
        node_engine,
        pnpm_version: ToolVersion::parse(pnpm_version, "packageManager")?,
        preferred_node,
    })
}

fn parse_version_component(value: Option<&str>, source: &str, original: &str) -> Result<u64> {
    value
        .ok_or_else(|| -> Box<dyn std::error::Error> {
            format!("{source} 不是有效的三段式版本号：{original}").into()
        })?
        .parse::<u64>()
        .map_err(|_| format!("{source} 不是有效的三段式版本号：{original}").into())
}

pub(crate) fn node_engine_satisfies(version: ToolVersion, engine: &str) -> Result<bool> {
    engine
        .split("||")
        .map(str::trim)
        .filter(|alternative| !alternative.is_empty())
        .map(|alternative| node_engine_alternative_satisfies(version, alternative))
        .collect::<Result<Vec<_>>>()
        .map(|results| results.into_iter().any(std::convert::identity))
}

fn node_engine_alternative_satisfies(version: ToolVersion, alternative: &str) -> Result<bool> {
    let comparators = alternative
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .filter(|comparator| !comparator.is_empty())
        .collect::<Vec<_>>();
    if comparators.is_empty() {
        return Err("engines.node 不能包含空的版本范围".into());
    }
    comparators
        .into_iter()
        .map(|comparator| node_comparator_satisfies(version, comparator))
        .collect::<Result<Vec<_>>>()
        .map(|results| results.into_iter().all(std::convert::identity))
}

fn node_comparator_satisfies(version: ToolVersion, comparator: &str) -> Result<bool> {
    if let Some(required) = comparator.strip_prefix('^') {
        let required = ToolVersion::parse(required, "engines.node")?;
        let upper_bound_matches = match (required.major, required.minor) {
            (0, 0) => version.major == 0 && version.minor == 0 && version.patch == required.patch,
            (0, _) => version.major == 0 && version.minor == required.minor,
            _ => version.major == required.major,
        };
        return Ok(version >= required && upper_bound_matches);
    }

    for operator in [">=", "<=", ">", "<", "="] {
        if let Some(required) = comparator.strip_prefix(operator) {
            let required = ToolVersion::parse(required, "engines.node")?;
            return Ok(match operator {
                ">=" => version >= required,
                "<=" => version <= required,
                ">" => version > required,
                "<" => version < required,
                "=" => version == required,
                _ => unreachable!("比较运算符来自固定集合"),
            });
        }
    }

    Ok(version == ToolVersion::parse(comparator, "engines.node")?)
}
