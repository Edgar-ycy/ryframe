use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Result, process::child_command};

use super::model::{
    CacheState, DevexRunOptions, DevexSuite, StepDefinition, SuiteDefinition, WorkingDirectory,
};

const ENVIRONMENT_WHITELIST: &[&str] = &[
    "CARGO_BUILD_JOBS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_INCREMENTAL",
    "CARGO_PROFILE_DEV_CODEGEN_UNITS",
    "CARGO_PROFILE_TEST_CODEGEN_UNITS",
    "CARGO_TARGET_DIR",
    "CI",
    "FORCE_COLOR",
    "NO_COLOR",
    "RUSTC_WRAPPER",
    "RUSTFLAGS",
    "SCCACHE_GHA_ENABLED",
    "SCCACHE_RECACHE",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct RunMetadata {
    pub(super) schema_version: u8,
    pub(super) run_id: String,
    pub(super) started_at: String,
    pub(super) suite: DevexSuite,
    pub(super) variant: String,
    pub(super) cache_state: CacheState,
    pub(super) requested_runs: usize,
    pub(super) backend: SourceState,
    pub(super) frontend: Option<SourceState>,
    pub(super) toolchain: Toolchain,
    pub(super) target: String,
    pub(super) features: Vec<String>,
    pub(super) jobs: usize,
    pub(super) environment: BTreeMap<String, String>,
    pub(super) environment_hash: String,
    pub(super) commands: Vec<CommandMetadata>,
    pub(super) compile_surface_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct SourceState {
    commit: Option<String>,
    dirty: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Toolchain {
    cargo: String,
    rustc: String,
    node: Option<String>,
    pnpm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct CommandMetadata {
    working_directory: String,
    program: String,
    args: Vec<String>,
}

pub(super) struct MetadataContext<'a> {
    pub(super) backend_root: &'a Path,
    pub(super) frontend_root: &'a Path,
    pub(super) devex_root: &'a Path,
    pub(super) run_id: &'a str,
    pub(super) options: &'a DevexRunOptions,
    pub(super) definition: SuiteDefinition,
    pub(super) effective_environment: &'a BTreeMap<String, String>,
}

pub(super) fn collect(context: MetadataContext<'_>) -> Result<RunMetadata> {
    let normalizer = PathNormalizer::new(
        context.backend_root,
        context.frontend_root,
        context.devex_root,
    );
    let frontend_suite = context
        .definition
        .steps
        .iter()
        .any(|step| step.working_directory == WorkingDirectory::Frontend);
    let toolchain = Toolchain {
        cargo: version(context.backend_root, "cargo", &["--version"]),
        rustc: version(context.backend_root, "rustc", &["-vV"]),
        node: frontend_suite.then(|| version(context.frontend_root, "node", &["--version"])),
        pnpm: frontend_suite.then(|| {
            version(
                context.frontend_root,
                corepack_executable(),
                &["pnpm", "--version"],
            )
        }),
    };
    let environment_hash = environment_hash(context.effective_environment);
    let environment = visible_environment(context.effective_environment, &normalizer);
    let commands = command_metadata(context.definition.steps);
    let target = rust_host(&toolchain.rustc).unwrap_or_else(|| env::consts::ARCH.to_owned());
    let jobs = context
        .effective_environment
        .get("CARGO_BUILD_JOBS")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(default_jobs);
    let compile_surface_fingerprint = compile_surface_fingerprint(
        context.backend_root,
        context.frontend_root,
        context.options.suite,
        &toolchain,
        &target,
        context.definition,
        &commands,
        &environment_hash,
        jobs,
        &normalizer,
    )?;

    Ok(RunMetadata {
        schema_version: 1,
        run_id: context.run_id.to_owned(),
        started_at: Utc::now().to_rfc3339(),
        suite: context.options.suite,
        variant: context.options.variant.clone(),
        cache_state: context.options.cache_state,
        requested_runs: context.options.runs,
        backend: source_state(context.backend_root),
        frontend: frontend_suite.then(|| source_state(context.frontend_root)),
        toolchain,
        target,
        features: context
            .definition
            .features
            .iter()
            .map(ToString::to_string)
            .collect(),
        jobs,
        environment,
        environment_hash,
        commands,
        compile_surface_fingerprint,
    })
}

pub(super) fn inherited_environment() -> BTreeMap<String, String> {
    filter_environment(env::vars_os())
}

pub(crate) fn filter_environment(
    values: impl IntoIterator<Item = (OsString, OsString)>,
) -> BTreeMap<String, String> {
    values
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            ENVIRONMENT_WHITELIST
                .contains(&key.as_str())
                .then(|| (key, value.to_string_lossy().into_owned()))
        })
        .collect()
}

fn visible_environment(
    environment: &BTreeMap<String, String>,
    normalizer: &PathNormalizer,
) -> BTreeMap<String, String> {
    environment
        .iter()
        .map(|(key, value)| {
            let value = if matches!(key.as_str(), "RUSTFLAGS" | "CARGO_ENCODED_RUSTFLAGS") {
                format!("{}（内容仅参与哈希）", sha256(value.as_bytes()))
            } else {
                normalizer.normalize_environment_value(value)
            };
            (key.clone(), value)
        })
        .collect()
}

fn environment_hash(environment: &BTreeMap<String, String>) -> String {
    sha256(&serde_json::to_vec(environment).expect("环境映射必须可序列化"))
}

fn command_metadata(steps: &[StepDefinition]) -> Vec<CommandMetadata> {
    steps
        .iter()
        .map(|step| CommandMetadata {
            working_directory: match step.working_directory {
                WorkingDirectory::Backend => "$BACKEND",
                WorkingDirectory::Frontend => "$FRONTEND",
            }
            .to_owned(),
            program: step.program.to_owned(),
            args: step
                .args
                .iter()
                .map(|arg| arg.replace("{target}", "$TARGET"))
                .collect(),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn compile_surface_fingerprint(
    backend_root: &Path,
    frontend_root: &Path,
    suite: DevexSuite,
    toolchain: &Toolchain,
    target: &str,
    definition: SuiteDefinition,
    commands: &[CommandMetadata],
    environment_hash: &str,
    jobs: usize,
    normalizer: &PathNormalizer,
) -> Result<String> {
    let frontend_suite = definition
        .steps
        .iter()
        .any(|step| step.working_directory == WorkingDirectory::Frontend);
    let cargo_metadata = (!frontend_suite)
        .then(|| normalized_cargo_metadata(backend_root, normalizer))
        .transpose()?;
    let inputs = if frontend_suite {
        input_hashes(frontend_root, FRONTEND_COMPILE_INPUTS)
    } else {
        rust_input_hashes(backend_root, cargo_metadata.as_ref())
    }?;
    let document = json!({
        "schema_version": 1,
        "suite": suite,
        "toolchain": toolchain,
        "target": target,
        "features": definition.features,
        "jobs": jobs,
        "environment_hash": environment_hash,
        "commands": commands,
        "cargo_metadata": cargo_metadata,
        "inputs": inputs,
    });
    Ok(sha256(&serde_json::to_vec(&document)?))
}

fn normalized_cargo_metadata(root: &Path, normalizer: &PathNormalizer) -> Result<Value> {
    let output = child_command("cargo")
        .args(["metadata", "--locked", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "无法读取 Cargo 编译面：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let mut document: Value = serde_json::from_slice(&output.stdout)?;
    normalize_json_strings(&mut document, normalizer);
    Ok(document)
}

fn normalize_json_strings(value: &mut Value, normalizer: &PathNormalizer) {
    match value {
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| normalize_json_strings(value, normalizer)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| normalize_json_strings(value, normalizer)),
        Value::String(value) => *value = normalizer.normalize(value),
        _ => {}
    }
}

fn rust_input_hashes(root: &Path, metadata: Option<&Value>) -> Result<BTreeMap<String, String>> {
    let mut paths = BTreeSet::from([
        root.join("Cargo.lock"),
        root.join("Cargo.toml"),
        root.join("rust-toolchain.toml"),
        root.join(".cargo/config.toml"),
    ]);
    if let Some(packages) = metadata
        .and_then(|value| value.get("packages"))
        .and_then(Value::as_array)
    {
        for manifest in packages.iter().filter_map(|package| {
            package
                .get("manifest_path")
                .and_then(Value::as_str)
                .map(|value| value.replace("$BACKEND", &root.to_string_lossy()))
        }) {
            paths.insert(PathBuf::from(manifest));
        }
    }
    let mut hashes = BTreeMap::new();
    for path in paths {
        if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            hashes.insert(relative, sha256(&fs::read(path)?));
        }
    }
    Ok(hashes)
}

const FRONTEND_COMPILE_INPUTS: &[&str] = &[
    ".node-version",
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "tsconfig.app.json",
    "tsconfig.base.json",
    "tsconfig.json",
    "tsconfig.test.json",
    "vite.config.ts",
    "vitest.config.ts",
];

fn input_hashes(root: &Path, paths: &[&str]) -> Result<BTreeMap<String, String>> {
    paths
        .iter()
        .filter_map(|relative| {
            let path = root.join(relative);
            path.is_file().then_some((*relative, path))
        })
        .map(|(relative, path)| Ok((relative.to_owned(), sha256(&fs::read(path)?))))
        .collect()
}

fn source_state(root: &Path) -> SourceState {
    SourceState {
        commit: git_text(root, &["rev-parse", "HEAD"]),
        dirty: git_text(root, &["status", "--porcelain", "--untracked-files=all"])
            .map(|status| !status.is_empty()),
    }
}

fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn version(root: &Path, program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn rust_host(rustc: &str) -> Option<String> {
    rustc
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(ToOwned::to_owned))
}

fn default_jobs() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

pub(super) fn corepack_executable() -> &'static str {
    if cfg!(windows) {
        "corepack.cmd"
    } else {
        "corepack"
    }
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut value = String::with_capacity(7 + digest.len() * 2);
    value.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write;
        write!(value, "{byte:02x}").expect("写入 String 不会失败");
    }
    value
}

#[derive(Debug, Clone)]
pub(crate) struct PathNormalizer {
    replacements: Vec<(String, &'static str)>,
}

impl PathNormalizer {
    pub(crate) fn new(backend: &Path, frontend: &Path, devex: &Path) -> Self {
        let mut replacements = vec![
            (normalized_path(devex), "$DEVEX"),
            (normalized_path(frontend), "$FRONTEND"),
            (normalized_path(backend), "$BACKEND"),
        ];
        replacements.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));
        Self { replacements }
    }

    pub(crate) fn normalize(&self, value: &str) -> String {
        let mut value = value.replace('\\', "/");
        for (path, replacement) in &self.replacements {
            value = value.replace(path, replacement);
        }
        value
    }

    fn normalize_environment_value(&self, value: &str) -> String {
        let normalized = self.normalize(value);
        if Path::new(&normalized).is_absolute() {
            let name = Path::new(&normalized)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("executable");
            format!("$EXTERNAL_PATH/{name}")
        } else {
            normalized
        }
    }
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .replace('\\', "/")
}
