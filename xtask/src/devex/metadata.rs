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

use super::model::{
    DevexSuite, StepDefinition, SuiteDefinition, WorkingDirectory, is_common_frontend_fast,
};

#[path = "metadata/path_normalizer.rs"]
mod path_normalizer;
#[path = "metadata/schema.rs"]
mod schema;
#[path = "metadata/source.rs"]
mod source;

pub(crate) use path_normalizer::PathNormalizer;
pub(crate) use schema::SourceFingerprints;
use schema::{CommandMetadata, SourceState, Toolchain};
pub(super) use schema::{MetadataContext, RunMetadata};
pub(super) use source::{
    collect_runtime_source_fingerprints, collect_source_fingerprints, source_state,
};

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
    let common = context.pairing.as_ref().is_some_and(|pairing| {
        pairing
            .baseline_contract
            .is_some_and(|contract| contract.uses_common_toolchain())
    });
    let toolchain = collect_toolchain(&context, frontend_suite || common, common)?;
    let comparable_environment = comparable_environment(context.effective_environment, &normalizer);
    let environment_hash = environment_hash(&comparable_environment);
    let environment = visible_environment(&comparable_environment);
    let commands = command_metadata(context.definition.steps, &context.options.variant)?;
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

    let (backend, frontend, runner_frontend) = collect_sources(&context, frontend_suite, common)?;
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
        runtime: None,
        pairing: context.pairing,
    })
}

fn collect_sources(
    context: &MetadataContext<'_>,
    frontend_suite: bool,
    common: bool,
) -> Result<(SourceState, Option<SourceState>, Option<SourceState>)> {
    let backend = source_state(context.backend_root)?;
    let frontend = (frontend_suite || common)
        .then(|| source_state(context.frontend_root))
        .transpose()?;
    if common
        && (backend.dirty != Some(false)
            || frontend.as_ref().and_then(|source| source.dirty) != Some(false))
    {
        return Err("共同 DevEx 的实际前后端测量工作树必须干净".into());
    }
    let runner_frontend = (context.options.suite.is_runtime()
        || context.definition.steps.iter().any(is_common_frontend_fast))
    .then(|| source_state(context.runner_frontend_root))
    .transpose()?;
    Ok((backend, frontend, runner_frontend))
}

fn collect_toolchain(
    context: &MetadataContext<'_>,
    frontend_suite: bool,
    common: bool,
) -> Result<Toolchain> {
    let frontend_tooling = if context.options.suite.is_runtime() {
        context.runner_frontend_root
    } else {
        context.frontend_root
    };
    let common = common
        .then(|| super::common_toolchain::capture(context.backend_root, context.frontend_root))
        .transpose()?;
    Ok(Toolchain {
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
        common,
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

pub(in crate::devex) fn command_metadata(
    steps: &[StepDefinition],
    variant: &str,
) -> Result<Vec<CommandMetadata>> {
    steps
        .iter()
        .map(|step| {
            Ok(CommandMetadata {
                working_directory: match step.working_directory {
                    WorkingDirectory::Backend => "$BACKEND",
                    WorkingDirectory::Frontend => "$FRONTEND",
                    WorkingDirectory::RunnerFrontend => "$RUNNER_FRONTEND",
                }
                .to_owned(),
                program: step.program.to_owned(),
                args: super::model::step_arguments(step, variant)?
                    .iter()
                    .map(|arg| command_argument(step, arg))
                    .collect(),
            })
        })
        .collect()
}

fn command_argument(step: &StepDefinition, arg: &str) -> String {
    if is_common_frontend_fast(step) {
        return arg
            .replace("{frontend-cache}", "$FRONTEND_CACHE")
            .replace("{runner-frontend}", "$RUNNER_FRONTEND")
            .replace("{frontend}", "$FRONTEND");
    }
    arg.replace("{backend}", "$BACKEND")
        .replace("{target}", "$TARGET")
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
    if suite == DevexSuite::ResourceGate {
        return 12;
    }
    if suite == DevexSuite::RustGate {
        return default_jobs().saturating_sub(2).clamp(4, 12);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_metadata_distinguishes_source_workspace_and_sample_target() {
        let definition = DevexSuite::CargoDevSave.definition("worker-only").unwrap();
        let commands = command_metadata(definition.steps, "worker-only").unwrap();
        assert_eq!(commands[0].working_directory, "$BACKEND");
        assert_eq!(
            commands[0].args,
            [
                "xtask",
                "dev",
                "--measure-once",
                "--workspace-root",
                "$BACKEND",
                "--case",
                "worker-only",
                "--output",
                "$TARGET/cargo-dev-save-result.json",
            ]
        );
    }
}
