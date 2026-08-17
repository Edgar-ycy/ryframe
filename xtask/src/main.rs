//! RyFrame 的跨仓库开发任务。
//!
//! 该命令刻意调用各仓库的标准命令而非重写检查逻辑，以保持本地开发与 CI 行为一致，
//! 同时允许覆盖前端目录。

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[cfg(windows)]
const COREPACK_EXECUTABLE: &str = "corepack.cmd";
#[cfg(not(windows))]
const COREPACK_EXECUTABLE: &str = "corepack";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ToolVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl ToolVersion {
    fn parse(value: &str, source: &str) -> Result<Self> {
        let value = value.trim().trim_start_matches('v');
        let mut parts = value.split('.');
        let major = parse_version_component(parts.next(), source, value)?;
        let minor = parse_version_component(parts.next(), source, value)?;
        let patch = parse_version_component(parts.next(), source, value)?;
        if parts.next().is_some() {
            return Err(format!("{source} 不是有效的三段式版本号: {value}").into());
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for ToolVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

struct FrontendToolchain {
    node_engine: String,
    pnpm_version: ToolVersion,
    preferred_node: ToolVersion,
}

const MINIMUM_PYTHON_VERSION: ToolVersion = ToolVersion {
    major: 3,
    minor: 11,
    patch: 0,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct FeatureMatrixEntry {
    package: String,
    minimal: Vec<String>,
    maximal: Vec<String>,
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let frontend_dir = take_option(&mut args, "--frontend-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| root_dir().join("ryframe-vue3"));
    let command = args.first().map(String::as_str).unwrap_or("help");

    match command {
        "doctor" => doctor(&frontend_dir),
        "check" => check(&args[1..], &frontend_dir),
        "contract" => contract(&args[1..], &frontend_dir),
        "feature-matrix" => feature_matrix(),
        "release-verify" => release_verify(&args[1..], &frontend_dir),
        "dev" => dev(&frontend_dir),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        unknown => Err(format!("unknown xtask command: {unknown}").into()),
    }
}

fn root_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live at the workspace root")
        .to_path_buf()
}

fn take_option(args: &mut Vec<String>, option: &str) -> Option<String> {
    let position = args.iter().position(|arg| arg == option)?;
    if position + 1 >= args.len() {
        eprintln!("{option} requires a value");
        std::process::exit(2);
    }
    args.remove(position);
    Some(args.remove(position))
}

fn doctor(frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    for executable in ["cargo", "rustc", "python", "git"] {
        run(&root, executable, &["--version"])?;
    }

    let python_version = python_version(&root)?;
    if python_version < MINIMUM_PYTHON_VERSION {
        return Err(format!(
            "Python {python_version} 低于后端脚本要求的 {}；请安装 Python 3.11+",
            MINIMUM_PYTHON_VERSION
        )
        .into());
    }

    for (name, path) in [
        ("backend Git repository", root.join(".git")),
        ("frontend directory", frontend_dir.to_path_buf()),
        ("frontend Git repository", frontend_dir.join(".git")),
        ("backend configuration", root.join("config/app.toml")),
    ] {
        if !path.exists() {
            return Err(format!("{name} is missing: {}", path.display()).into());
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
    let pnpm_version = corepack_pnpm_version(&root)?;
    if pnpm_version != toolchain.pnpm_version {
        return Err(format!(
            "pnpm {pnpm_version} 与前端 packageManager=pnpm@{} 不一致；请通过 corepack 使用固定版本",
            toolchain.pnpm_version
        )
        .into());
    }

    println!("RyFrame developer environment is ready.");
    Ok(())
}

fn check(args: &[String], frontend_dir: &Path) -> Result<()> {
    let scope = option_value(args, "--scope").unwrap_or("all");
    match scope {
        "backend" => backend_check(),
        "frontend" => frontend_check(frontend_dir),
        "all" => {
            backend_check()?;
            frontend_check(frontend_dir)
        }
        _ => Err("--scope must be one of all, backend, frontend".into()),
    }
}

fn backend_check() -> Result<()> {
    let root = root_dir();
    check_feature_registry(&root)?;
    run(&root, "cargo", &["fmt", "--all", "--", "--check"])?;
    run(
        &root,
        "cargo",
        &["check", "--locked", "--workspace", "--lib", "--bins"],
    )?;
    run(
        &root,
        "cargo",
        &[
            "clippy",
            "--locked",
            "--workspace",
            "--lib",
            "--bins",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    for script in [
        "scripts/check_architecture.py",
        "scripts/check_prerelease_dependencies.py",
        "scripts/check_permission_routes.py",
    ] {
        run(&root, "python", &[script])?;
    }
    Ok(())
}

fn feature_matrix() -> Result<()> {
    let root = root_dir();
    let metadata = load_workspace_metadata(&root)?;
    let registry = load_feature_registry(&root)?;
    validate_feature_registry(&metadata, &registry)?;

    for entry in &registry {
        run_feature_combination(&root, &entry.package, "最小", &entry.minimal)?;
        if entry.minimal != entry.maximal {
            run_feature_combination(&root, &entry.package, "最大", &entry.maximal)?;
        }
    }
    Ok(())
}

fn check_feature_registry(root: &Path) -> Result<()> {
    let metadata = load_workspace_metadata(root)?;
    let registry = load_feature_registry(root)?;
    validate_feature_registry(&metadata, &registry)?;
    println!("Cargo feature 注册表检查通过。");
    Ok(())
}

fn load_feature_registry(root: &Path) -> Result<Vec<FeatureMatrixEntry>> {
    let path = root.join("config/feature-matrix.json");
    let source = fs::read(&path)
        .map_err(|error| format!("无法读取 feature 注册表 {}: {error}", path.display()))?;
    let document: serde_json::Value = serde_json::from_slice(&source)
        .map_err(|error| format!("feature 注册表不是有效 JSON: {error}"))?;
    if document.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err("feature 注册表 version 必须为 1".into());
    }
    let packages = document
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("feature 注册表缺少 packages 数组")?;

    packages
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let package = entry
                .get("package")
                .and_then(serde_json::Value::as_str)
                .filter(|package| !package.trim().is_empty())
                .ok_or_else(|| format!("feature 注册表 packages[{index}] 缺少 package"))?;
            Ok(FeatureMatrixEntry {
                package: package.to_owned(),
                minimal: feature_names(entry.get("minimal"), index, "minimal")?,
                maximal: feature_names(entry.get("maximal"), index, "maximal")?,
            })
        })
        .collect()
}

fn feature_names(
    value: Option<&serde_json::Value>,
    index: usize,
    field: &str,
) -> Result<Vec<String>> {
    value
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("feature 注册表 packages[{index}].{field} 必须是数组"))?
        .iter()
        .enumerate()
        .map(|(feature_index, value)| {
            value
                .as_str()
                .filter(|feature| !feature.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "feature 注册表 packages[{index}].{field}[{feature_index}] 必须是非空字符串"
                    )
                    .into()
                })
        })
        .collect()
}

fn load_workspace_metadata(root: &Path) -> Result<serde_json::Value> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(format!("无法读取 Cargo 工作区元数据: {stderr}").into());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Cargo 工作区元数据不是有效 JSON: {error}").into())
}

fn workspace_features(metadata: &serde_json::Value) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let members = metadata
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo 元数据缺少 workspace_members")?
        .iter()
        .map(|member| {
            member
                .as_str()
                .map(str::to_owned)
                .ok_or("Cargo workspace_members 中存在非字符串成员")
        })
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo 元数据缺少 packages")?;
    let mut result = BTreeMap::new();

    for package in packages {
        let id = package
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or("Cargo package 缺少 id")?;
        if !members.contains(id) {
            continue;
        }
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or("Cargo package 缺少 name")?;
        let features = package
            .get("features")
            .and_then(serde_json::Value::as_object)
            .ok_or("Cargo package 缺少 features")?
            .keys()
            .cloned()
            .collect();
        if result.insert(name.to_owned(), features).is_some() {
            return Err(format!("Cargo 工作区存在重复包名: {name}").into());
        }
    }

    if result.len() != members.len() {
        return Err("Cargo 元数据未包含全部工作区成员".into());
    }
    Ok(result)
}

fn validate_feature_registry(
    metadata: &serde_json::Value,
    registry: &[FeatureMatrixEntry],
) -> Result<()> {
    let packages = workspace_features(metadata)?;
    let feature_packages = packages
        .iter()
        .filter(|(_, features)| !features.is_empty())
        .map(|(name, _)| name.as_str())
        .collect::<BTreeSet<_>>();
    let mut registered_packages = BTreeSet::new();

    for entry in registry {
        if !registered_packages.insert(entry.package.as_str()) {
            return Err(format!("Cargo feature 注册表重复登记包: {}", entry.package).into());
        }
        let available = packages
            .get(entry.package.as_str())
            .ok_or_else(|| format!("Cargo feature 注册表包含未知包: {}", entry.package))?;
        if available.is_empty() {
            return Err(format!("包 {} 没有 feature，不应登记矩阵", entry.package).into());
        }

        let minimal =
            validate_feature_combination(&entry.package, "最小", &entry.minimal, available)?;
        let maximal =
            validate_feature_combination(&entry.package, "最大", &entry.maximal, available)?;
        if !minimal.is_subset(&maximal) {
            return Err(
                format!("包 {} 的最小 feature 组合不是最大组合的子集", entry.package).into(),
            );
        }
        if &maximal != available {
            let missing = available.difference(&maximal).cloned().collect::<Vec<_>>();
            return Err(format!(
                "包 {} 的最大 feature 组合未覆盖: {}",
                entry.package,
                missing.join(", ")
            )
            .into());
        }
    }

    if registered_packages != feature_packages {
        let missing = feature_packages
            .difference(&registered_packages)
            .copied()
            .collect::<Vec<_>>();
        let extra = registered_packages
            .difference(&feature_packages)
            .copied()
            .collect::<Vec<_>>();
        return Err(format!(
            "Cargo feature 注册表与工作区不一致；未登记: [{}]，多余: [{}]",
            missing.join(", "),
            extra.join(", ")
        )
        .into());
    }
    Ok(())
}

fn validate_feature_combination(
    package: &str,
    label: &str,
    combination: &[String],
    available: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let selected = combination.iter().cloned().collect::<BTreeSet<_>>();
    if selected.len() != combination.len() {
        return Err(format!("包 {package} 的{label} feature 组合包含重复项").into());
    }
    let unknown = selected.difference(available).cloned().collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(format!(
            "包 {package} 的{label} feature 组合包含未知项: {}",
            unknown.join(", ")
        )
        .into());
    }
    Ok(selected)
}

fn run_feature_combination(
    root: &Path,
    package: &str,
    label: &str,
    features: &[String],
) -> Result<()> {
    println!("检查 {package} 的{label} feature 组合。");
    let feature_list = features.join(",");
    let mut args = vec!["check", "--locked", "-p", package, "--no-default-features"];
    if !feature_list.is_empty() {
        args.extend(["--features", feature_list.as_str()]);
    }
    run(root, "cargo", &args)
}

fn frontend_check(frontend_dir: &Path) -> Result<()> {
    run_pnpm(frontend_dir, &["check"])
}

fn contract(args: &[String], frontend_dir: &Path) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("check") => run_pnpm(frontend_dir, &["api:check"]),
        Some("sync") => run_pnpm(frontend_dir, &["api:sync"]),
        _ => Err("usage: cargo xtask contract <check|sync> [--frontend-dir PATH]".into()),
    }
}

fn release_verify(args: &[String], frontend_dir: &Path) -> Result<()> {
    let tag = required_option(args, "--tag")?;
    let backend_repository = required_option(args, "--backend-repository")?;
    let backend_commit = required_option(args, "--backend-commit")?;
    let frontend_repository = required_option(args, "--frontend-repository")?;
    let frontend_commit = required_option(args, "--frontend-commit")?;
    let manifest_path = required_option(args, "--manifest-path")?;
    let frontend = frontend_dir
        .canonicalize()
        .map_err(|error| format!("cannot resolve frontend directory: {error}"))?;
    let root = root_dir();
    run(
        &root,
        "python",
        &[
            "scripts/validate_release.py",
            "--tag",
            tag,
            "--frontend-dir",
            frontend.to_str().ok_or("frontend path is not UTF-8")?,
            "--backend-repository",
            backend_repository,
            "--backend-commit",
            backend_commit,
            "--frontend-repository",
            frontend_repository,
            "--frontend-commit",
            frontend_commit,
            "--manifest-path",
            manifest_path,
        ],
    )
}

fn dev(frontend_dir: &Path) -> Result<()> {
    doctor(frontend_dir)?;
    let root = root_dir();
    println!("Starting backend and frontend. Press Ctrl+C to stop both processes.");
    let mut backend = spawn(&root, "cargo", &["run"])?;
    let mut frontend = match spawn_pnpm(frontend_dir, &["dev"]) {
        Ok(child) => child,
        Err(error) => {
            let _ = stop_child(&mut backend);
            return Err(error);
        }
    };
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let _shutdown_listener = spawn_shutdown_listener(shutdown_requested.clone());

    loop {
        if shutdown_requested.load(Ordering::SeqCst) {
            println!("收到中断信号，正在停止后端和前端进程。");
            stop_children(&mut backend, &mut frontend)?;
            return Ok(());
        }
        if let Some(status) = backend.try_wait()? {
            stop_child(&mut frontend)?;
            return Err(format!("backend exited with {status}").into());
        }
        if let Some(status) = frontend.try_wait()? {
            stop_child(&mut backend)?;
            return Err(format!("frontend exited with {status}").into());
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn option_value<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == option)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn required_option<'a>(args: &'a [String], option: &str) -> Result<&'a str> {
    option_value(args, option).ok_or_else(|| {
        format!(
            "usage: cargo xtask release-verify --tag vMAJOR.MINOR.PATCH \\
             --backend-repository OWNER/REPO --backend-commit SHA \\
             --frontend-repository OWNER/REPO --frontend-commit SHA \\
             --manifest-path PATH (missing {option})"
        )
        .into()
    })
}

fn run(dir: &Path, executable: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(executable)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed: {executable} {}", args.join(" ")).into())
    }
}

fn run_pnpm(dir: &Path, args: &[&str]) -> Result<()> {
    let status = pnpm_command(dir)?
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed: pnpm {}", args.join(" ")).into())
    }
}

fn command_version(dir: &Path, executable: &str) -> Result<ToolVersion> {
    let output = command_version_output(dir, executable)?;
    ToolVersion::parse(&output, &format!("{executable} --version"))
}

fn corepack_pnpm_version(dir: &Path) -> Result<ToolVersion> {
    let output = pnpm_command(dir)?.arg("--version").output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(format!("无法通过项目级 Corepack shim 获取 pnpm 版本: {stderr}").into());
    }
    let output = String::from_utf8(output.stdout)
        .map_err(|error| format!("pnpm --version 输出不是 UTF-8: {error}"))?;
    ToolVersion::parse(&output, "项目级 pnpm --version")
}

fn pnpm_command(dir: &Path) -> Result<Command> {
    let executable = pnpm_executable(dir)?;
    let shim_dir = executable
        .parent()
        .expect("pnpm Corepack shim must have a parent directory");
    let inherited_path = env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![shim_dir.to_path_buf()];
    paths.extend(env::split_paths(&inherited_path));
    let path = env::join_paths(paths)
        .map_err(|error| format!("无法构造 pnpm 的 PATH 环境变量: {error}"))?;

    let mut command = Command::new(executable);
    command.current_dir(dir).env("PATH", path);
    Ok(command)
}

fn pnpm_executable(dir: &Path) -> Result<PathBuf> {
    let shim_dir = root_dir().join("target").join("corepack-bin");
    fs::create_dir_all(&shim_dir).map_err(|error| {
        format!(
            "无法创建 Corepack shim 目录 {}: {error}",
            shim_dir.display()
        )
    })?;

    #[cfg(windows)]
    let executable = shim_dir.join("pnpm.cmd");
    #[cfg(not(windows))]
    let executable = shim_dir.join("pnpm");

    if !executable.is_file() {
        let status = Command::new(COREPACK_EXECUTABLE)
            .args(["enable", "--install-directory"])
            .arg(&shim_dir)
            .current_dir(dir)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        if !status.success() {
            return Err("无法创建项目级 Corepack pnpm shim".into());
        }
    }

    if !executable.is_file() {
        return Err(format!("Corepack 未创建 pnpm shim: {}", executable.display()).into());
    }
    Ok(executable)
}

fn python_version(dir: &Path) -> Result<ToolVersion> {
    let output = command_version_output(dir, "python")?;
    parse_python_version(&output)
}

fn parse_python_version(output: &str) -> Result<ToolVersion> {
    let version = output
        .trim()
        .strip_prefix("Python ")
        .ok_or_else(|| format!("python --version 输出不是预期格式: {}", output.trim()))?;
    ToolVersion::parse(version, "python --version")
}

fn command_version_output(dir: &Path, executable: &str) -> Result<String> {
    let output = Command::new(executable)
        .arg("--version")
        .current_dir(dir)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(format!("无法获取 {executable} 版本: {stderr}").into());
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("{executable} --version 输出不是 UTF-8: {error}").into())
}

fn frontend_toolchain(frontend_dir: &Path) -> Result<FrontendToolchain> {
    let package_path = frontend_dir.join("package.json");
    let package_source = fs::read_to_string(&package_path)
        .map_err(|error| format!("无法读取 {}: {error}", package_path.display()))?;
    let package: serde_json::Value = serde_json::from_str(&package_source)
        .map_err(|error| format!("{} 不是有效 JSON: {error}", package_path.display()))?;
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
        .map_err(|error| format!("无法读取 {}: {error}", preferred_node_path.display()))?;
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
        .ok_or_else(|| -> Box<dyn Error> {
            format!("{source} 不是有效的三段式版本号: {original}").into()
        })?
        .parse::<u64>()
        .map_err(|_| format!("{source} 不是有效的三段式版本号: {original}").into())
}

fn node_engine_satisfies(version: ToolVersion, engine: &str) -> Result<bool> {
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

fn spawn_shutdown_listener(shutdown_requested: Arc<AtomicBool>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("无法安装 Ctrl+C 监听器: {error}");
                return;
            }
        };
        if runtime.block_on(tokio::signal::ctrl_c()).is_ok() {
            shutdown_requested.store(true, Ordering::SeqCst);
        }
    })
}

fn stop_children(
    backend: &mut std::process::Child,
    frontend: &mut std::process::Child,
) -> Result<()> {
    let backend_result = stop_child(backend);
    let frontend_result = stop_child(frontend);
    backend_result?;
    frontend_result
}

fn stop_child(child: &mut std::process::Child) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }

    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let status = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() && child.try_wait()?.is_none() {
            child.kill()?;
        }
    }
    #[cfg(unix)]
    {
        signal_process_group(child.id(), libc::SIGTERM)?;
        if wait_for_child_exit(child, Duration::from_secs(5))? {
            return Ok(());
        }

        signal_process_group(child.id(), libc::SIGKILL)?;
        if !wait_for_child_exit(child, Duration::from_secs(5))? {
            child.kill()?;
        }
    }

    let _ = child.wait();
    Ok(())
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: libc::c_int) -> Result<()> {
    let pid = i32::try_from(pid).map_err(|_| "child process ID exceeds POSIX range")?;
    // POSIX 规定负 PID 表示整个进程组；子进程在 spawn 时成为自己的进程组组长。
    let result = unsafe { libc::kill(-pid, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(format!("cannot signal child process group {pid}: {error}").into())
}

#[cfg(unix)]
fn wait_for_child_exit(child: &mut std::process::Child, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn spawn(dir: &Path, executable: &str, args: &[&str]) -> Result<std::process::Child> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // 为每个开发服务建立独立进程组，停止时能同时回收其派生子进程。
        command.process_group(0);
    }
    Ok(command.spawn()?)
}

fn spawn_pnpm(dir: &Path, args: &[&str]) -> Result<std::process::Child> {
    let mut command = pnpm_command(dir)?;
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // 为每个开发服务建立独立进程组，停止时能同时回收其派生子进程。
        command.process_group(0);
    }
    Ok(command.spawn()?)
}

fn print_help() {
    println!(
        "RyFrame workspace tasks\n\n\
         cargo xtask doctor [--frontend-dir PATH]\n\
         cargo xtask check [--scope all|backend|frontend] [--frontend-dir PATH]\n\
         cargo xtask contract <check|sync> [--frontend-dir PATH]\n\
         cargo xtask feature-matrix\n\
         cargo xtask release-verify --tag vMAJOR.MINOR.PATCH \\
             --backend-repository OWNER/REPO --backend-commit SHA \\
             --frontend-repository OWNER/REPO --frontend-commit SHA \\
             --manifest-path PATH [--frontend-dir PATH]\n\
         cargo xtask dev [--frontend-dir PATH]"
    );
}
