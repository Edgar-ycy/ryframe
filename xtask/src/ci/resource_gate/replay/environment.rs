use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value;

use super::{path::normalized_path, sha256_json};

pub(super) const FIXED_VERIFY_JOBS: &str = "12";
pub(super) const FIXED_RESOURCE_GATE_TEST_JOBS: &str = "8";
const RUST_ENVIRONMENT_KEYS: [&str; 18] = [
    "CARGO_BUILD_JOBS",
    "CARGO_BUILD_TARGET",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_INCREMENTAL",
    "CARGO_TARGET_DIR",
    "RUSTC",
    "RUSTC_BOOTSTRAP",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTC_WRAPPER",
    "RUST_BACKTRACE",
    "RUST_LIB_BACKTRACE",
    "RUST_LOG",
    "RUST_MIN_STACK",
    "RUST_TEST_THREADS",
    "RUSTDOC",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
];
const REMOVED_ENVIRONMENT_KEYS: [&str; 7] = [
    "RYFRAME_DEVEX_TARGET_ROOT",
    "RYFRAME_RESOURCE_GATE_DECISION_FILE",
    "RYFRAME_RESOURCE_GATE_TEST_JOBS",
    "RYFRAME_RESOURCE_GATE_TARGETED",
    "RYFRAME_VERIFY_JOBS",
    "CMAKE_C_COMPILER_LAUNCHER",
    "CMAKE_CXX_COMPILER_LAUNCHER",
];

#[derive(Debug, Clone)]
pub(crate) struct ToolExecutables {
    pub(crate) corepack: PathBuf,
    pub(crate) sccache: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct ReplayTools {
    pub(crate) executables: ToolExecutables,
    pub(crate) versions: BTreeMap<String, String>,
}

pub(crate) type ProcessEnvironment = BTreeMap<String, OsString>;

pub(crate) fn resolve_tool_executables() -> Result<ToolExecutables, String> {
    Ok(ToolExecutables {
        corepack: required_executable(if cfg!(windows) {
            "corepack.cmd"
        } else {
            "corepack"
        })?,
        sccache: required_executable(if cfg!(windows) {
            "sccache.exe"
        } else {
            "sccache"
        })?,
    })
}

pub(crate) fn collect_tools(
    backend: &Path,
    frontend: &Path,
    executables: ToolExecutables,
    environment: &ProcessEnvironment,
) -> Result<ReplayTools, String> {
    let mut versions = BTreeMap::new();
    versions.insert("xtask".to_owned(), env!("CARGO_PKG_VERSION").to_owned());
    versions.insert(
        "git".to_owned(),
        tool_version(backend, "git", &["--version"], environment)?,
    );
    versions.insert(
        "cargo".to_owned(),
        tool_version(backend, "cargo", &["--version"], environment)?,
    );
    versions.insert(
        "rustc".to_owned(),
        tool_version(backend, "rustc", &["-vV"], environment)?,
    );
    versions.insert(
        "node".to_owned(),
        tool_version(frontend, "node", &["--version"], environment)?,
    );
    versions.insert(
        "corepack".to_owned(),
        tool_version(frontend, &executables.corepack, &["--version"], environment)?,
    );
    versions.insert(
        "pnpm".to_owned(),
        tool_version(
            frontend,
            &executables.corepack,
            &["pnpm", "--version"],
            environment,
        )?,
    );
    versions.insert(
        "sccache".to_owned(),
        tool_version(backend, &executables.sccache, &["--version"], environment)?,
    );
    Ok(ReplayTools {
        executables,
        versions,
    })
}

pub(crate) fn base_environment() -> ProcessEnvironment {
    env::vars_os()
        .filter_map(|(key, value)| key.into_string().ok().map(|key| (key, value)))
        .collect()
}

pub(crate) fn controlled_environment(
    mut environment: ProcessEnvironment,
    sccache: &Path,
    cache: &Path,
    activation_gate: bool,
) -> Result<ProcessEnvironment, String> {
    if activation_gate {
        for name in [
            "RYFRAME_MYSQL_INTEGRATION",
            "RYFRAME_MYSQL_TLS_INTEGRATION",
            "RYFRAME_REDIS_INTEGRATION",
        ] {
            if text_value(&environment, name) != Some("1") {
                return Err(format!("activation replay 要求 {name}=1 并准备隔离服务"));
            }
        }
        for name in ["RYFRAME_MYSQL_HOST", "RYFRAME_REDIS_HOST"] {
            if !matches!(
                text_value(&environment, name),
                Some("127.0.0.1" | "localhost" | "::1")
            ) {
                return Err(format!("activation replay 要求 {name} 显式使用回环地址"));
            }
        }
        if text_value(&environment, "RYFRAME_REDIS_DATABASE") != Some("15") {
            return Err("activation replay 要求 RYFRAME_REDIS_DATABASE=15".to_owned());
        }
    }
    let remove = environment
        .keys()
        .filter(|key| {
            key.starts_with("RYFRAME_CI_")
                || key.starts_with("SCCACHE_")
                || RUST_ENVIRONMENT_KEYS.contains(&key.as_str())
                || REMOVED_ENVIRONMENT_KEYS.contains(&key.as_str())
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in remove {
        environment.remove(&key);
    }
    for (key, value) in [
        ("CI", "1"),
        ("CARGO_INCREMENTAL", "0"),
        ("CARGO_TERM_COLOR", "never"),
        (
            "RYFRAME_RESOURCE_GATE_TEST_JOBS",
            FIXED_RESOURCE_GATE_TEST_JOBS,
        ),
        ("RYFRAME_VERIFY_JOBS", FIXED_VERIFY_JOBS),
    ] {
        environment.insert(key.to_owned(), OsString::from(value));
    }
    environment.insert("RUSTC_WRAPPER".to_owned(), sccache.as_os_str().to_owned());
    environment.insert("SCCACHE_DIR".to_owned(), cache.as_os_str().to_owned());
    environment.insert(
        "SCCACHE_SERVER_PORT".to_owned(),
        OsString::from(available_port()?.to_string()),
    );
    Ok(environment)
}

pub(crate) fn recorded_environment(
    environment: &ProcessEnvironment,
    activation_gate: bool,
) -> Result<BTreeMap<String, String>, String> {
    let mut recorded = BTreeMap::new();
    for key in [
        "CI",
        "CARGO_INCREMENTAL",
        "CARGO_TERM_COLOR",
        "RYFRAME_RESOURCE_GATE_TEST_JOBS",
        "RYFRAME_VERIFY_JOBS",
    ] {
        recorded.insert(
            key.to_owned(),
            required_text_value(environment, key)?.to_owned(),
        );
    }
    recorded.insert("RUSTC_WRAPPER".to_owned(), "sccache".to_owned());
    recorded.insert("SCCACHE_BACKEND".to_owned(), "dedicated-local".to_owned());
    recorded.insert("activationGate".to_owned(), activation_gate.to_string());
    recorded.insert(
        "frontendInstall".to_owned(),
        "corepack pnpm install --offline --frozen-lockfile".to_owned(),
    );
    recorded.insert(
        "mysqlIntegration".to_owned(),
        text_value(environment, "RYFRAME_MYSQL_INTEGRATION")
            .unwrap_or("0")
            .to_owned(),
    );
    recorded.insert(
        "mysqlTlsIntegration".to_owned(),
        text_value(environment, "RYFRAME_MYSQL_TLS_INTEGRATION")
            .unwrap_or("0")
            .to_owned(),
    );
    recorded.insert(
        "redisIntegration".to_owned(),
        text_value(environment, "RYFRAME_REDIS_INTEGRATION")
            .unwrap_or("0")
            .to_owned(),
    );
    if let Some(value) = text_value(environment, "SCCACHE_BASEDIRS") {
        let basedirs = env::split_paths(OsStr::new(value))
            .map(|path| normalized_path(&path))
            .collect::<Vec<_>>();
        if !basedirs.is_empty() {
            recorded.insert(
                "SCCACHE_BASEDIRS_COUNT".to_owned(),
                basedirs.len().to_string(),
            );
            recorded.insert(
                "SCCACHE_BASEDIRS_FINGERPRINT".to_owned(),
                sha256_json(&basedirs)?,
            );
        }
    }
    Ok(recorded)
}

pub(crate) fn tool_version(
    root: &Path,
    executable: impl AsRef<OsStr>,
    arguments: &[&str],
    environment: &ProcessEnvironment,
) -> Result<String, String> {
    let output = Command::new(executable.as_ref())
        .args(arguments)
        .current_dir(root)
        .env_clear()
        .envs(environment)
        .output()
        .map_err(|error| {
            format!(
                "无法读取工具版本 {}：{error}",
                executable.as_ref().to_string_lossy()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "无法读取工具版本 {}：{}",
            executable.as_ref().to_string_lossy(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(crate) fn ensure_sccache_server(
    sccache: &Path,
    environment: &ProcessEnvironment,
) -> Result<(), String> {
    match run_sccache(sccache, environment, "--start-server") {
        Ok(()) => Ok(()),
        Err(error) if error.contains("Timed out waiting for server startup") => {
            sccache_stats(sccache, environment).map(|_| ())
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn reset_sccache_stats(
    sccache: &Path,
    environment: &ProcessEnvironment,
) -> Result<(), String> {
    run_sccache(sccache, environment, "--zero-stats")
}

pub(crate) fn sccache_stats(
    sccache: &Path,
    environment: &ProcessEnvironment,
) -> Result<Value, String> {
    let output = Command::new(sccache)
        .args(["--show-stats", "--stats-format", "json"])
        .env_clear()
        .envs(environment)
        .output()
        .map_err(|error| format!("sccache 统计无法启动：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "sccache 统计失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let document = serde_json::from_slice::<Value>(&output.stdout)
        .map_err(|error| format!("sccache 统计不是合法 JSON：{error}"))?;
    if !document.is_object() {
        return Err("sccache 统计必须是 JSON 对象".to_owned());
    }
    Ok(document)
}

pub(crate) fn sccache_error_count(document: &Value) -> Result<u64, String> {
    let stats = document
        .get("stats")
        .and_then(Value::as_object)
        .ok_or_else(|| "sccache 统计缺少 stats 对象".to_owned())?;
    let mut errors = count_map(stats.get("cache_errors"), "stats.cache_errors")?;
    for name in [
        "cache_timeouts",
        "cache_read_errors",
        "cache_write_errors",
        "dist_errors",
    ] {
        errors += non_negative_int(stats.get(name), &format!("stats.{name}"))?;
    }
    Ok(errors)
}

fn count_map(value: Option<&Value>, label: &str) -> Result<u64, String> {
    let counts = value
        .and_then(Value::as_object)
        .and_then(|value| value.get("counts"))
        .and_then(Value::as_object)
        .ok_or_else(|| format!("sccache {label}.counts 必须是对象"))?;
    counts.iter().try_fold(0_u64, |total, (name, value)| {
        Ok(total + non_negative_int(Some(value), &format!("{label}.counts.{name}"))?)
    })
}

fn non_negative_int(value: Option<&Value>, label: &str) -> Result<u64, String> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("sccache {label} 必须是非负整数"))
}

pub(crate) fn stop_sccache(sccache: &Path, environment: &ProcessEnvironment) -> Option<String> {
    run_sccache(sccache, environment, "--stop-server").err()
}

fn run_sccache(
    sccache: &Path,
    environment: &ProcessEnvironment,
    argument: &str,
) -> Result<(), String> {
    let output = Command::new(sccache)
        .arg(argument)
        .env_clear()
        .envs(environment)
        .output()
        .map_err(|error| format!("sccache {argument} 无法启动：{error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "sccache {argument} 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn required_executable(name: &str) -> Result<PathBuf, String> {
    let path = env::var_os("PATH").ok_or_else(|| "resource replay 缺少 PATH".to_owned())?;
    let candidates = executable_candidates(name);
    for directory in env::split_paths(&path) {
        for candidate in &candidates {
            let path = directory.join(candidate);
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    Err(format!(
        "resource replay 缺少可执行文件：{name}；请安装并将其目录加入 PATH"
    ))
}

fn executable_candidates(name: &str) -> Vec<String> {
    if cfg!(windows) {
        let path = Path::new(name);
        if path.extension().is_some() {
            return vec![name.to_owned()];
        }
        let extensions = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
        std::iter::once(name.to_owned())
            .chain(
                extensions
                    .split(';')
                    .map(|extension| format!("{name}{}", extension.to_ascii_lowercase())),
            )
            .collect()
    } else {
        vec![name.to_owned()]
    }
}

fn available_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| format!("无法分配 replay sccache 端口：{error}"))
}

fn text_value<'a>(environment: &'a ProcessEnvironment, key: &str) -> Option<&'a str> {
    environment.get(key).and_then(|value| value.to_str())
}

fn required_text_value<'a>(
    environment: &'a ProcessEnvironment,
    key: &str,
) -> Result<&'a str, String> {
    text_value(environment, key).ok_or_else(|| format!("replay 环境变量 {key} 缺失或不是 UTF-8"))
}
