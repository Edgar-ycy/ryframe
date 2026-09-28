use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use chrono::Utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Result, process::child_command};

use super::model::{DevexSuite, StepDefinition, SuiteDefinition, WorkingDirectory};

#[path = "metadata/path_normalizer.rs"]
mod path_normalizer;
#[path = "metadata/schema.rs"]
mod schema;

pub(crate) use path_normalizer::PathNormalizer;
pub(crate) use schema::SourceFingerprints;
use schema::{CommandMetadata, SourceState, Toolchain};
pub(super) use schema::{MetadataContext, RunMetadata};

const ENVIRONMENT_WHITELIST: &[&str] = &[
    "CC",
    "CARGO_BUILD_JOBS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_INCREMENTAL",
    "CARGO_NET_OFFLINE",
    "CARGO_PROFILE_DEV_CODEGEN_UNITS",
    "CARGO_PROFILE_TEST_CODEGEN_UNITS",
    "CARGO_TARGET_DIR",
    "CFLAGS",
    "CI",
    "CMAKE_CXX_COMPILER_LAUNCHER",
    "CMAKE_C_COMPILER_LAUNCHER",
    "CXX",
    "CXXFLAGS",
    "FORCE_COLOR",
    "NO_COLOR",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTFLAGS",
    "RYFRAME_DEVEX_SAVE_CASE",
    "RYFRAME_DEVEX_TARGET_ROOT",
    "RYFRAME_FAST_CHECK_CACHE_ROOT",
    "RYFRAME_CI_BASE_SHA",
    "RYFRAME_CI_FRONTEND_REF",
    "RYFRAME_CI_HEAD_SHA",
    "RYFRAME_CI_RUST_GATE_PROFILE",
    "RYFRAME_CI_TEST_JOBS",
    "RYFRAME_RESOURCE_GATE_TEST_JOBS",
    "RYFRAME_VERIFY_JOBS",
    "GITHUB_BASE_SHA",
    "GITHUB_SHA",
    "SCCACHE_BASEDIRS",
    "SCCACHE_CACHE_SIZE",
    "SCCACHE_CLIENT_SIDE",
    "SCCACHE_C_CUSTOM_CACHE_BUSTER",
    "SCCACHE_DIR",
    "SCCACHE_DIRECT",
    "SCCACHE_ENDPOINT",
    "SCCACHE_ERROR_LOG",
    "SCCACHE_GHA_ENABLED",
    "SCCACHE_GHA_VERSION",
    "SCCACHE_IDLE_TIMEOUT",
    "SCCACHE_IGNORE_SERVER_IO_ERROR",
    "SCCACHE_LOG",
    "SCCACHE_NO_DAEMON",
    "SCCACHE_RECACHE",
    "SCCACHE_SERVER_PORT",
    "SCCACHE_SERVER_UDS",
];

pub(super) fn collect(context: MetadataContext<'_>) -> Result<RunMetadata> {
    let normalizer = PathNormalizer::new(
        context.backend_root,
        context.frontend_root,
        context.devex_root,
    );
    let frontend_suite = context.definition.requires_frontend;
    let toolchain = collect_toolchain(&context, frontend_suite);
    let comparable_environment = comparable_environment(context.effective_environment, &normalizer);
    let environment_hash = environment_hash(&comparable_environment);
    let environment = visible_environment(&comparable_environment);
    let commands = command_metadata(context.definition.steps);
    let target = rust_host(&toolchain.rustc).unwrap_or_else(|| env::consts::ARCH.to_owned());
    let jobs = effective_jobs(context.options.suite, context.effective_environment);
    let compile_surface_fingerprint = compile_surface_fingerprint(
        context.options.suite,
        &context.options.variant,
        &toolchain,
        &target,
        context.definition,
        &commands,
        context
            .pairing
            .as_ref()
            .and_then(|pairing| pairing.workload_contract.as_ref()),
        &environment_hash,
        jobs,
    )?;
    let input_fingerprint = input_fingerprint(
        context.backend_root,
        context.frontend_root,
        context.definition,
        &normalizer,
    )?;

    let backend = source_state(context.backend_root)?;
    let frontend = frontend_suite
        .then(|| source_state(context.frontend_root))
        .transpose()?;
    let runner_frontend = context
        .options
        .suite
        .is_runtime()
        .then(|| source_state(context.runner_frontend_root))
        .transpose()?;
    Ok(RunMetadata {
        schema_version: 1,
        run_id: context.run_id.to_owned(),
        started_at: Utc::now().to_rfc3339(),
        suite: context.options.suite,
        variant: context.options.variant.clone(),
        cache_state: context.options.cache_state,
        requested_runs: context.options.runs,
        backend,
        frontend,
        runner_frontend,
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
        input_fingerprint,
        pairing: context.pairing,
    })
}

fn collect_toolchain(context: &MetadataContext<'_>, frontend_suite: bool) -> Toolchain {
    let frontend_tooling = if context.options.suite.is_runtime() {
        context.runner_frontend_root
    } else {
        context.frontend_root
    };
    Toolchain {
        cargo: version(context.backend_root, "cargo", &["--version"]),
        rustc: version(context.backend_root, "rustc", &["-vV"]),
        sccache: context
            .options
            .suite
            .uses_sccache()
            .then(|| version(context.backend_root, "sccache", &["--version"])),
        node: frontend_suite.then(|| version(frontend_tooling, "node", &["--version"])),
        pnpm: frontend_suite.then(|| {
            version(
                frontend_tooling,
                corepack_executable(),
                &["pnpm", "--version"],
            )
        }),
    }
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

fn comparable_environment(
    environment: &BTreeMap<String, String>,
    normalizer: &PathNormalizer,
) -> BTreeMap<String, String> {
    environment
        .iter()
        .map(|(key, value)| {
            let value = if key == "SCCACHE_DIR" {
                "$RUN_CACHE/sccache".to_owned()
            } else if key == "SCCACHE_SERVER_PORT" {
                "$EPHEMERAL_PORT".to_owned()
            } else {
                normalizer.normalize_environment_value(value)
            };
            (key.clone(), value)
        })
        .collect()
}

fn visible_environment(environment: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    environment
        .iter()
        .map(|(key, value)| {
            let value = if matches!(key.as_str(), "RUSTFLAGS" | "CARGO_ENCODED_RUSTFLAGS") {
                format!("{}（内容仅参与哈希）", sha256(value.as_bytes()))
            } else {
                value.clone()
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
                WorkingDirectory::RunnerFrontend => "$RUNNER_FRONTEND",
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
    suite: DevexSuite,
    variant: &str,
    toolchain: &Toolchain,
    target: &str,
    definition: SuiteDefinition,
    commands: &[CommandMetadata],
    workload_contract: Option<&super::model::PairedWorkloadContract>,
    environment_hash: &str,
    jobs: usize,
) -> Result<String> {
    let execution = workload_contract.map_or_else(
        || json!({"commands": commands}),
        |contract| json!({"paired_workload_contract": contract}),
    );
    let document = json!({
        "schema_version": 1,
        "suite": suite,
        "variant": variant,
        "toolchain": toolchain,
        "target": target,
        "features": definition.features,
        "jobs": jobs,
        "environment_hash": environment_hash,
        "execution": execution,
    });
    Ok(sha256(&serde_json::to_vec(&document)?))
}

fn input_fingerprint(
    backend_root: &Path,
    frontend_root: &Path,
    definition: SuiteDefinition,
    normalizer: &PathNormalizer,
) -> Result<String> {
    let cargo_metadata = normalized_cargo_metadata(backend_root, normalizer)?;
    let rust_inputs = rust_input_hashes(backend_root, Some(&cargo_metadata))?;
    let frontend_inputs = definition
        .requires_frontend
        .then(|| input_hashes(frontend_root, FRONTEND_COMPILE_INPUTS))
        .transpose()?;
    Ok(sha256(&serde_json::to_vec(&json!({
        "cargo_metadata": cargo_metadata,
        "rust_inputs": rust_inputs,
        "frontend_inputs": frontend_inputs,
    }))?))
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

fn source_state(root: &Path) -> Result<SourceState> {
    let commit = git_text(root, &["rev-parse", "HEAD"])
        .ok_or_else(|| format!("无法读取源码提交：{}", root.display()))?;
    let status = git_text(root, &["status", "--porcelain", "--untracked-files=all"])
        .ok_or_else(|| format!("无法读取源码状态：{}", root.display()))?;
    Ok(SourceState {
        commit: Some(commit.clone()),
        dirty: Some(!status.is_empty()),
        worktree_fingerprint: worktree_fingerprint(root, &commit)?,
    })
}

pub(super) fn collect_source_fingerprints(
    backend_root: &Path,
    frontend_root: Option<&Path>,
) -> Result<SourceFingerprints> {
    Ok(SourceFingerprints {
        backend: source_state(backend_root)?.worktree_fingerprint,
        frontend: frontend_root
            .map(source_state)
            .transpose()?
            .map(|state| state.worktree_fingerprint),
        runner_frontend: None,
    })
}

pub(super) fn collect_runtime_source_fingerprints(
    backend_root: &Path,
    frontend_root: &Path,
    runner_frontend_root: Option<&Path>,
) -> Result<SourceFingerprints> {
    let mut fingerprints = collect_source_fingerprints(backend_root, Some(frontend_root))?;
    fingerprints.runner_frontend = runner_frontend_root
        .map(source_state)
        .transpose()?
        .map(|state| state.worktree_fingerprint);
    Ok(fingerprints)
}

impl RunMetadata {
    pub(super) fn source_fingerprints(&self) -> SourceFingerprints {
        SourceFingerprints {
            backend: self.backend.worktree_fingerprint.clone(),
            frontend: self
                .frontend
                .as_ref()
                .map(|state| state.worktree_fingerprint.clone()),
            runner_frontend: self
                .runner_frontend
                .as_ref()
                .map(|state| state.worktree_fingerprint.clone()),
        }
    }
}

fn worktree_fingerprint(root: &Path, commit: &str) -> Result<String> {
    let tracked = git_output(
        root,
        &["diff", "--binary", "--no-ext-diff", "HEAD", "--", "."],
    )?;
    let untracked = git_output(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut digest = Sha256::new();
    update_digest(&mut digest, commit.as_bytes());
    update_digest(&mut digest, &tracked);
    for raw_path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = std::str::from_utf8(raw_path)
            .map_err(|_| "未跟踪文件路径不是 UTF-8，无法建立源码指纹")?;
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(format!("Git 返回越界的未跟踪路径：{relative}").into());
        }
        update_digest(&mut digest, raw_path);
        update_digest(&mut digest, &fs::read(root.join(relative_path))?);
    }
    Ok(format_digest(digest.finalize()))
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "git {} 失败：{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into())
    }
}

fn update_digest(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
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

fn effective_jobs(suite: DevexSuite, environment: &BTreeMap<String, String>) -> usize {
    if suite == DevexSuite::RustGate {
        return environment
            .get("RYFRAME_VERIFY_JOBS")
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| default_jobs().saturating_sub(2).clamp(4, 12));
    }
    environment
        .get("CARGO_BUILD_JOBS")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(default_jobs)
}

pub(super) fn corepack_executable() -> &'static str {
    if cfg!(windows) {
        "corepack.cmd"
    } else {
        "corepack"
    }
}

fn sha256(bytes: &[u8]) -> String {
    format_digest(Sha256::digest(bytes))
}

fn format_digest(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut value = String::with_capacity(7 + digest.len() * 2);
    value.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write;
        write!(value, "{byte:02x}").expect("写入 String 不会失败");
    }
    value
}
