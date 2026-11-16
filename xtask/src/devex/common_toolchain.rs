use std::{
    env, fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    process::{child_command, command_output_result_with_timeout, pnpm_cache_argument},
};

const RUST: &str = "1.98.1";
const NODE: &str = "v24.21.0";
const PNPM: &str = "12.8.1";
const CACHE_ARGUMENT: &str = "--config.cache-dir=";
const COMPILER_OVERRIDES: [&str; 7] = [
    "RUSTC",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTDOC",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTDOC",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CommonToolchain {
    pub(super) rustup_toolchain: String,
    rustup: ToolFile,
    cargo: RustTool,
    rustc: RustTool,
    node: VersionedTool,
    corepack: VersionedTool,
    pnpm: VersionedTool,
    pnpm_node: PathBuf,
    pub(super) corepack_home: PathBuf,
    pub(super) pnpm_cache_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RustTool {
    proxy_path: PathBuf,
    proxy_copy: Option<ToolFile>,
    actual: VersionedTool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct VersionedTool {
    file: ToolFile,
    version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ToolFile {
    path: PathBuf,
    canonical_path: PathBuf,
    bytes: u64,
    modified: SystemTime,
    sha256: String,
}

pub(super) fn capture(backend: &Path, frontend: &Path) -> Result<CommonToolchain> {
    check_compiler_environment()?;
    let rustup_toolchain = selected_toolchain()?;
    let corepack_home = selected_home()?;
    let cache_argument = pnpm_cache_argument(backend)?;
    let pnpm_cache_dir = cache_argument.as_deref().map(cache_directory).transpose()?;

    let rustup = ToolFile::capture(path_program(rustup_name())?)?;
    let cargo = RustTool::capture(backend, &rustup, &rustup_toolchain, "cargo", cargo_name())?;
    let rustc = RustTool::capture(backend, &rustup, &rustup_toolchain, "rustc", rustc_name())?;
    let node = capture_node(frontend)?;
    let corepack_file = ToolFile::capture(path_program(super::metadata::corepack_executable())?)?;
    let mut corepack_version = corepack_command(frontend, &corepack_file.path, &corepack_home);
    corepack_version.arg("--version");
    let corepack = VersionedTool {
        version: output(corepack_version, "Corepack 版本")?,
        file: corepack_file,
    };
    let (pnpm, pnpm_node) = capture_pnpm(
        frontend,
        &corepack.file.path,
        &corepack_home,
        cache_argument.as_deref(),
        &node.file,
    )?;
    let toolchain = CommonToolchain {
        rustup_toolchain,
        rustup,
        cargo,
        rustc,
        node,
        corepack,
        pnpm,
        pnpm_node,
        corepack_home,
        pnpm_cache_dir,
    };
    toolchain.check_files()?;
    Ok(toolchain)
}

impl CommonToolchain {
    pub(super) fn check_files(&self) -> Result<()> {
        check_compiler_environment()?;
        if selected_toolchain()? != self.rustup_toolchain || selected_home()? != self.corepack_home
        {
            return Err("DevEx 共同 RUSTUP_TOOLCHAIN 或 COREPACK_HOME 发生漂移".into());
        }
        for (name, expected) in [
            (rustup_name(), &self.rustup.path),
            (cargo_name(), &self.cargo.proxy_path),
            (rustc_name(), &self.rustc.proxy_path),
            (node_name(), &self.node.file.path),
            (
                super::metadata::corepack_executable(),
                &self.corepack.file.path,
            ),
        ] {
            let actual = path_program(name)?;
            if !same_file::is_same_file(&actual, expected)? {
                return Err(format!("DevEx 共同工具 PATH 绑定发生漂移：{name}").into());
            }
        }
        for tool in [&self.cargo, &self.rustc] {
            let proxy = tool.proxy_copy.as_ref().unwrap_or(&self.rustup);
            if !same_file::is_same_file(&tool.proxy_path, &proxy.path)? {
                return Err(
                    format!("DevEx Rustup 代理发生漂移：{}", tool.proxy_path.display()).into(),
                );
            }
        }
        if !same_file::is_same_file(&self.pnpm_node, &self.node.file.path)? {
            return Err("DevEx pnpm 的 Node 路径与共同 Node 发生漂移".into());
        }
        if let Some(cache) = &self.pnpm_cache_dir {
            ordinary_directory(cache)?;
        }
        for file in self.files() {
            file.check()?;
        }
        Ok(())
    }

    pub(super) fn verify_digests(&self) -> Result<()> {
        self.check_files()?;
        for file in self.files() {
            file.verify_digest()?;
        }
        self.check_files()
    }

    fn files(&self) -> impl Iterator<Item = &ToolFile> {
        [
            &self.rustup,
            &self.cargo.actual.file,
            &self.rustc.actual.file,
            &self.node.file,
            &self.corepack.file,
            &self.pnpm.file,
        ]
        .into_iter()
        .chain(self.cargo.proxy_copy.as_ref())
        .chain(self.rustc.proxy_copy.as_ref())
    }
}

impl RustTool {
    fn capture(
        root: &Path,
        rustup: &ToolFile,
        toolchain: &str,
        name: &str,
        program: &str,
    ) -> Result<Self> {
        let proxy_path = path_program(program)?;
        let proxy_copy = if same_file::is_same_file(&proxy_path, &rustup.path)? {
            None
        } else {
            let file = ToolFile::capture(proxy_path.canonicalize()?)?;
            if file.sha256 != rustup.sha256 {
                return Err(format!("DevEx PATH 的 {name} 不是已绑定 Rustup 的代理").into());
            }
            Some(file)
        };
        let resolved = output(
            probe(
                root,
                &rustup.path,
                &["which", "--toolchain", toolchain, name],
            ),
            &format!("rustup which {name}"),
        )?;
        let file = ToolFile::capture(PathBuf::from(resolved))?;
        let version = output(probe(root, &file.path, &["--version"]), name)?;
        let proxy_version = output(probe(root, &proxy_path, &["--version"]), name)?;
        if version != proxy_version || !rust_version(&version, name) {
            return Err(format!("DevEx {name} 代理版本与 Rustup {RUST} 安装不一致").into());
        }
        Ok(Self {
            proxy_path,
            proxy_copy,
            actual: VersionedTool { file, version },
        })
    }
}

fn capture_node(frontend: &Path) -> Result<VersionedTool> {
    let program = path_program(node_name())?;
    let resolved = output(
        probe(frontend, &program, &["-p", "process.execPath"]),
        "Node 实际路径",
    )?;
    let file = ToolFile::capture(PathBuf::from(resolved))?;
    let version = output(probe(frontend, &program, &["--version"]), "Node 版本")?;
    if version != NODE || !same_file::is_same_file(&program, &file.path)? {
        return Err(format!("DevEx PATH Node 必须与实际 Node {NODE} 同一文件").into());
    }
    Ok(VersionedTool { file, version })
}

fn capture_pnpm(
    frontend: &Path,
    corepack: &Path,
    home: &Path,
    cache_argument: Option<&str>,
    node: &ToolFile,
) -> Result<(VersionedTool, PathBuf)> {
    let command = |args: &[&str]| {
        let mut command = corepack_command(frontend, corepack, home);
        command.arg("pnpm");
        if let Some(argument) = cache_argument {
            command.arg(argument);
        }
        command.args(args);
        command
    };
    let version = output(command(&["--version"]), "Corepack pnpm 版本")?;
    if version != PNPM {
        return Err(format!("DevEx 共同 Corepack 必须选中 pnpm {PNPM}").into());
    }
    let resolve = |expression| {
        output(command(&["exec", "node", "-p", expression]), expression).map(PathBuf::from)
    };
    let file = ToolFile::capture(resolve("process.env.npm_execpath")?)?;
    let canonical_home = home.canonicalize()?;
    let installation = home.join("v1/pnpm").join(PNPM).canonicalize()?;
    if !installation.starts_with(canonical_home) || !file.canonical_path.starts_with(installation) {
        return Err("DevEx 实际 pnpm 程序必须来自已登记 COREPACK_HOME 的固定版本安装".into());
    }
    let pnpm_node = resolve("process.execPath")?;
    if !absolute_path(&pnpm_node) || !same_file::is_same_file(&pnpm_node, &node.path)? {
        return Err("DevEx Corepack pnpm 调用的 Node 与共同 Node 文件不一致".into());
    }
    Ok((VersionedTool { file, version }, pnpm_node))
}

fn probe(root: &Path, path: &Path, args: &[&str]) -> Command {
    let mut command = child_command(path);
    command
        .current_dir(root)
        .args(args)
        .env("NODE_DISABLE_COMPILE_CACHE", "1");
    command
}

fn corepack_command(root: &Path, launcher: &Path, home: &Path) -> Command {
    let mut command = probe(root, launcher, &[]);
    command
        .env("COREPACK_HOME", home)
        .env("COREPACK_ENABLE_NETWORK", "0")
        .env("COREPACK_DEFAULT_TO_LATEST", "0")
        .env("COREPACK_ENABLE_AUTO_PIN", "0")
        .env("COREPACK_ENV_FILE", "0")
        .env("npm_config_offline", "true")
        .env("pnpm_config_verify_deps_before_run", "error");
    command
}

fn output(mut command: Command, label: &str) -> Result<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let result = command_output_result_with_timeout(command, Duration::from_secs(30))?;
    let diagnostic = || format!("DevEx 无法得到有效的 {label}；核对固定工具和已有离线安装");
    if !result.status.success() || result.stdout.len() > 4096 {
        return Err(diagnostic().into());
    }
    let value = std::str::from_utf8(&result.stdout)
        .map_err(|_| diagnostic())?
        .trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(diagnostic().into());
    }
    Ok(value.to_owned())
}

fn check_compiler_environment() -> Result<()> {
    for name in COMPILER_OVERRIDES {
        if env::var_os(name).is_some() {
            return Err(format!("DevEx 共同工具链拒绝编译器环境覆盖：{name}").into());
        }
    }
    if let Some(wrapper) = env::var_os("RUSTC_WRAPPER")
        && !wrapper.is_empty()
        && Path::new(&wrapper)
            .file_stem()
            .is_none_or(|name| !name.eq_ignore_ascii_case("sccache"))
    {
        return Err("DevEx RUSTC_WRAPPER 只允许既有 suite 使用的 sccache".into());
    }
    Ok(())
}

fn selected_toolchain() -> Result<String> {
    let value = env::var("RUSTUP_TOOLCHAIN").map_err(|_| "DevEx 缺少明确的 RUSTUP_TOOLCHAIN")?;
    if !pinned_toolchain(&value) {
        return Err(format!("DevEx RUSTUP_TOOLCHAIN 必须显式固定为 {RUST}").into());
    }
    Ok(value)
}

fn pinned_toolchain(value: &str) -> bool {
    value == RUST
        || value
            .strip_prefix(RUST)
            .and_then(|value| value.strip_prefix('-'))
            .is_some_and(|host| {
                host.split('-').count() >= 3
                    && host.split('-').all(|part| {
                        !part.is_empty()
                            && part
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    })
            })
}

fn rust_version(value: &str, name: &str) -> bool {
    value.split_whitespace().take(2).eq([name, RUST])
}

fn selected_home() -> Result<PathBuf> {
    let path = env::var_os("COREPACK_HOME")
        .map(PathBuf::from)
        .ok_or("DevEx 缺少明确的 COREPACK_HOME")?;
    ordinary_directory(&path)?;
    Ok(path)
}

fn cache_directory(argument: &str) -> Result<PathBuf> {
    let path = PathBuf::from(
        argument
            .strip_prefix(CACHE_ARGUMENT)
            .ok_or("DevEx pnpm 缓存参数无效")?,
    );
    ordinary_directory(&path)?;
    Ok(path)
}

fn path_program(name: &str) -> Result<PathBuf> {
    let value = env::var_os("PATH").ok_or("DevEx 共同工具链缺少 PATH")?;
    for directory in env::split_paths(&value) {
        let path = directory.join(name);
        if path.is_file() {
            if !absolute_path(&path) {
                return Err(format!("DevEx PATH 命中的工具必须使用绝对路径：{name}").into());
            }
            return Ok(path);
        }
    }
    Err(format!("DevEx PATH 缺少已安装的共同工具：{name}").into())
}

const fn cargo_name() -> &'static str {
    if cfg!(windows) { "cargo.exe" } else { "cargo" }
}
const fn rustc_name() -> &'static str {
    if cfg!(windows) { "rustc.exe" } else { "rustc" }
}
const fn rustup_name() -> &'static str {
    if cfg!(windows) {
        "rustup.exe"
    } else {
        "rustup"
    }
}
const fn node_name() -> &'static str {
    if cfg!(windows) { "node.exe" } else { "node" }
}

fn absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|path| !path.chars().any(char::is_control))
        && !path.components().any(|part| part == Component::ParentDir)
}

fn ordinary_directory(path: &Path) -> Result<()> {
    if !absolute_path(path)
        || !fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir() && !linked(&meta))
    {
        return Err(format!("DevEx 工具目录必须是普通绝对路径：{}", path.display()).into());
    }
    Ok(())
}

fn ordinary_file(path: &Path) -> Result<fs::Metadata> {
    if absolute_path(path) {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.is_file() && !linked(&metadata) && metadata.len() != 0 {
            return Ok(metadata);
        }
    }
    Err(format!("DevEx 工具必须是非空普通绝对文件：{}", path.display()).into())
}

fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x0400 != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

impl ToolFile {
    fn capture(path: PathBuf) -> Result<Self> {
        let metadata = ordinary_file(&path)?;
        let file = Self {
            canonical_path: path.canonicalize()?,
            sha256: file_digest(&path)?,
            bytes: metadata.len(),
            modified: metadata.modified()?,
            path,
        };
        file.check()?;
        Ok(file)
    }

    fn check(&self) -> Result<()> {
        let metadata = ordinary_file(&self.path)?;
        if metadata.len() != self.bytes
            || metadata.modified()? != self.modified
            || self.path.canonicalize()? != self.canonical_path
        {
            return Err(format!("DevEx 共同工具文件发生漂移：{}", self.path.display()).into());
        }
        Ok(())
    }

    fn verify_digest(&self) -> Result<()> {
        if file_digest(&self.path)? != self.sha256 {
            return Err(format!(
                "DevEx 共同工具文件 SHA-256 发生漂移：{}",
                self.path.display()
            )
            .into());
        }
        Ok(())
    }
}

fn file_digest(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 32 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rustup_requires_the_exact_pin_and_matching_reported_version() {
        assert!(pinned_toolchain("1.98.1"));
        assert!(pinned_toolchain("1.98.1-x86_64-pc-windows-msvc"));
        for invalid in [
            "1.98.0",
            "stable",
            "1.98.10",
            "1.98.1-",
            "1.98.1-test-with-?",
        ] {
            assert!(!pinned_toolchain(invalid));
        }
        assert!(rust_version("rustc 1.98.1 (abcdef)", "rustc"));
        assert!(!rust_version("rustc 1.98.10", "rustc"));
    }

    #[test]
    fn metadata_checks_reject_size_drift_and_full_digest_rejects_same_size_edits() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("installed-tool.exe");
        fs::write(&path, b"first").unwrap();
        let captured = ToolFile::capture(path.clone()).unwrap();
        assert!(captured.check().is_ok());
        assert!(captured.verify_digest().is_ok());
        fs::write(&path, b"other").unwrap();
        assert!(captured.verify_digest().is_err());
        fs::write(&path, b"longer").unwrap();
        assert!(captured.check().is_err());
    }
}
